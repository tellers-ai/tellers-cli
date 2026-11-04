//! Talking to the agent: send with `POST /agent/messages`, follow the run on
//! the user websocket (`/users/ws`, `subscribe_chat`), and read a finished
//! turn's outcome with `GET /agent/chat/{id}/result`.
//!
//! The backend decides what a message does: start a turn on an idle chat, or
//! join the one running (between two tool calls, or by steering it). Its
//! `tellers.user_message` events say when this message was taken; the turn
//! that takes it is the one followed, up to its `tellers.stream.end`.

use futures_util::{SinkExt, StreamExt};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{json, Value};
use tellers_api_client::apis::configuration::Configuration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

/// What `POST /agent/messages` answers.
#[derive(Debug, Clone, Deserialize)]
pub struct Sent {
    pub chat_id: String,
    pub pending_id: String,
    /// Stream position to follow the run from, without a gap.
    pub stream_cursor: String,
}

/// Credentials as the rest of the CLI resolves them: an API key (may be empty
/// when signed in with `tellers login`) and/or a bearer header.
pub struct Auth<'a> {
    pub api_key: &'a str,
    pub bearer: Option<&'a str>,
}

fn with_auth(req: reqwest::RequestBuilder, auth: &Auth<'_>) -> reqwest::RequestBuilder {
    let mut req = req;
    if !auth.api_key.is_empty() {
        req = req.header("x-api-key", auth.api_key);
    }
    if let Some(bearer) = auth.bearer {
        req = req.header(AUTHORIZATION, bearer);
    }
    req
}

async fn error_from(resp: reqwest::Response) -> String {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    format!("request failed: status {} body: {}", status, body)
}

/// Sends `body` (an agent message request) to `POST /agent/messages`.
pub async fn send_message(cfg: &Configuration, auth: &Auth<'_>, body: &Value) -> Result<Sent, String> {
    let url = format!("{}/agent/messages", cfg.base_path);
    let resp = with_auth(reqwest::Client::new().post(url), auth)
        .header(CONTENT_TYPE, "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e))?;
    if !resp.status().is_success() {
        return Err(error_from(resp).await);
    }
    resp.json::<Sent>()
        .await
        .map_err(|e| format!("invalid response: {}", e))
}

/// The outcome of the chat's last turn, waiting for it to end.
///
/// Polls `GET /agent/chat/{id}/result`, which holds each request up to
/// `wait_seconds` while the turn runs, until `status` is no longer
/// `progressing`.
pub async fn wait_for_result(
    cfg: &Configuration,
    auth: &Auth<'_>,
    chat_id: &str,
) -> Result<Value, String> {
    let url = format!("{}/agent/chat/{}/result", cfg.base_path, chat_id);
    let client = reqwest::Client::new();
    loop {
        let resp = with_auth(client.get(&url), auth)
            .query(&[("wait_seconds", "60")])
            .send()
            .await
            .map_err(|e| format!("request failed: {}", e))?;
        if !resp.status().is_success() {
            return Err(error_from(resp).await);
        }
        let result: Value = resp
            .json()
            .await
            .map_err(|e| format!("invalid response: {}", e))?;
        if result.get("status").and_then(Value::as_str) != Some("progressing") {
            return Ok(result);
        }
    }
}

/// `https://api…` → `wss://api…/users/ws` (and `http` → `ws`).
pub fn users_ws_url(base_path: &str) -> String {
    let base = base_path.trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{}", rest)
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{}", rest)
    } else {
        base.to_string()
    };
    format!("{}/users/ws", ws_base)
}

/// Where a followed turn stands, given one more event of the chat.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Not this message's turn yet (the turn it queued behind).
    Skip,
    /// An event of the turn that took the message.
    Show,
    /// That turn's last event.
    End,
    /// The message was dropped before reaching the agent (a cancel).
    Dropped,
}

/// Follows which events belong to the turn that takes `pending_id`.
pub struct TurnFilter {
    pending_id: String,
    joined: bool,
}

impl TurnFilter {
    pub fn new(pending_id: &str) -> Self {
        Self {
            pending_id: pending_id.to_string(),
            joined: false,
        }
    }

    pub fn step(&mut self, event: &str, data: &Value) -> Step {
        if event == "tellers.user_message"
            && data.get("pending_id").and_then(Value::as_str) == Some(self.pending_id.as_str())
        {
            match data.get("status").and_then(Value::as_str) {
                Some("applied") | Some("started") => self.joined = true,
                Some("cancelled") | Some("failed") if !self.joined => return Step::Dropped,
                _ => {}
            }
        }
        if !self.joined {
            return Step::Skip;
        }
        if event == "tellers.stream.end" {
            Step::End
        } else {
            Step::Show
        }
    }
}

/// Follows the turn that takes the message `sent`, calling `on_event` with
/// each of its events (`event` name, `data`), up to its `tellers.stream.end`.
pub async fn follow_turn<F>(
    cfg: &Configuration,
    auth: &Auth<'_>,
    sent: &Sent,
    mut on_event: F,
) -> Result<(), String>
where
    F: FnMut(&str, &Value),
{
    let mut request = users_ws_url(&cfg.base_path)
        .into_client_request()
        .map_err(|e| format!("invalid websocket url: {}", e))?;
    let headers = request.headers_mut();
    if !auth.api_key.is_empty() {
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(auth.api_key).map_err(|e| e.to_string())?,
        );
    }
    if let Some(bearer) = auth.bearer {
        headers.insert(
            "authorization",
            HeaderValue::from_str(bearer).map_err(|e| e.to_string())?,
        );
    }
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| format!("websocket connection failed: {}", e))?;

    socket
        .send(Message::Text(
            json!({
                "action": "subscribe_chat",
                "chat_id": sent.chat_id,
                "after": sent.stream_cursor,
            })
            .to_string(),
        ))
        .await
        .map_err(|e| format!("websocket send failed: {}", e))?;

    let mut filter = TurnFilter::new(&sent.pending_id);
    let outcome = loop {
        let frame = match socket.next().await {
            Some(Ok(frame)) => frame,
            Some(Err(e)) => break Err(format!("websocket error: {}", e)),
            None => break Err("websocket closed before the turn ended".to_string()),
        };
        let text = match frame {
            Message::Text(text) => text,
            Message::Close(_) => break Err("websocket closed before the turn ended".to_string()),
            _ => continue,
        };
        let Ok(message) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if message.get("chat_id").and_then(Value::as_str) != Some(sent.chat_id.as_str()) {
            continue;
        }
        match message.get("type").and_then(Value::as_str) {
            Some("chat_subscribe_error") => {
                let reason = message.get("reason").and_then(Value::as_str).unwrap_or("unknown");
                break Err(format!("cannot follow the chat: {}", reason));
            }
            Some("chat_event") => {
                let event = message.get("event").and_then(Value::as_str).unwrap_or("");
                let data = message.get("data").cloned().unwrap_or(Value::Null);
                match filter.step(event, &data) {
                    Step::Skip => {}
                    Step::Show => on_event(event, &data),
                    Step::End => {
                        on_event(event, &data);
                        break Ok(());
                    }
                    Step::Dropped => break Err("the message was cancelled before the agent took it".to_string()),
                }
            }
            _ => {}
        }
    };

    let _ = socket
        .send(Message::Text(
            json!({"action": "unsubscribe_chat", "chat_id": sent.chat_id}).to_string(),
        ))
        .await;
    let _ = socket.close(None).await;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_url_follows_the_api_scheme() {
        assert_eq!(
            users_ws_url("https://api.prod.aws.tellers.ai"),
            "wss://api.prod.aws.tellers.ai/users/ws"
        );
        assert_eq!(users_ws_url("http://localhost:8000/"), "ws://localhost:8000/users/ws");
    }

    fn user_message(pending_id: &str, status: &str) -> Value {
        json!({"pending_id": pending_id, "status": status, "message": "hi"})
    }

    #[test]
    fn a_started_message_is_followed_to_its_turn_end() {
        let mut filter = TurnFilter::new("p-1");
        assert_eq!(filter.step("tellers.user_message", &user_message("p-1", "started")), Step::Show);
        assert_eq!(filter.step("response.output_text.delta", &json!({"delta": "Hi"})), Step::Show);
        assert_eq!(filter.step("tellers.stream.end", &json!({"reason": "completed"})), Step::End);
    }

    #[test]
    fn a_queued_message_skips_the_turn_it_waits_behind() {
        let mut filter = TurnFilter::new("p-2");
        assert_eq!(filter.step("tellers.user_message", &user_message("p-2", "queued")), Step::Skip);
        assert_eq!(filter.step("response.output_text.delta", &json!({"delta": "x"})), Step::Skip);
        // The running turn ends before taking it: still not ours.
        assert_eq!(filter.step("tellers.stream.end", &json!({})), Step::Skip);
        assert_eq!(filter.step("tellers.user_message", &user_message("p-2", "started")), Step::Show);
        assert_eq!(filter.step("tellers.stream.end", &json!({})), Step::End);
    }

    #[test]
    fn a_message_applied_mid_turn_joins_that_turn() {
        let mut filter = TurnFilter::new("p-3");
        assert_eq!(filter.step("response.output_text.delta", &json!({})), Step::Skip);
        assert_eq!(filter.step("tellers.user_message", &user_message("p-3", "applied")), Step::Show);
        assert_eq!(filter.step("tellers.stream.end", &json!({})), Step::End);
    }

    #[test]
    fn a_dropped_message_ends_the_follow() {
        let mut filter = TurnFilter::new("p-4");
        assert_eq!(filter.step("tellers.user_message", &user_message("p-4", "cancelled")), Step::Dropped);
    }

    #[test]
    fn another_messages_events_do_not_join() {
        let mut filter = TurnFilter::new("p-5");
        assert_eq!(filter.step("tellers.user_message", &user_message("other", "started")), Step::Skip);
    }
}
