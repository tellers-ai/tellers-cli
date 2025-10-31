use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use std::io::{self, Write};
use std::sync::mpsc;
use std::thread;
use tellers_api_client::apis::configuration::Configuration;
use tellers_api_client::models::AgentMessageRequest;

pub fn run_interactive(prompt_text: String, _full_auto: bool) -> Result<(), String> {
    let api_base = std::env::var("TELLERS_API_BASE")
        .unwrap_or_else(|_| "https://api.prod.aws.tellers.ai".to_string());
    let api_key =
        std::env::var("TELLERS_API_KEY").map_err(|_| "TELLERS_API_KEY not set".to_string())?;
    let bearer = std::env::var("TELLERS_AUTH_BEARER")
        .ok()
        .filter(|v| !v.is_empty());
    let bearer_header = bearer.as_deref().map(|b| {
        if b.starts_with("Bearer ") {
            b.to_string()
        } else {
            format!("Bearer {}", b)
        }
    });

    let mut cfg = Configuration::default();
    cfg.base_path = api_base;

    stream_and_print(
        &cfg,
        &api_key,
        bearer_header.as_deref(),
        prompt_text.clone(),
    )?;

    // Simple REPL: user can reply; Ctrl-C exits process
    let mut stdout = io::stdout();
    let stdin = io::stdin();
    loop {
        print!("\nYou: ");
        let _ = stdout.flush();
        let mut line = String::new();
        if stdin.read_line(&mut line).is_err() {
            break;
        }
        let message = line.trim().to_string();
        if message.is_empty() {
            continue;
        }
        stream_and_print(&cfg, &api_key, bearer_header.as_deref(), message)?;
    }
    Ok(())
}

pub fn run_background(prompt_text: String, _full_auto: bool) -> Result<String, String> {
    let api_base = std::env::var("TELLERS_API_BASE")
        .unwrap_or_else(|_| "https://api.prod.aws.tellers.ai".to_string());
    let api_key =
        std::env::var("TELLERS_API_KEY").map_err(|_| "TELLERS_API_KEY not set".to_string())?;
    let bearer = std::env::var("TELLERS_AUTH_BEARER")
        .ok()
        .filter(|v| !v.is_empty());
    let bearer_header = bearer.as_deref().map(|b| {
        if b.starts_with("Bearer ") {
            b.to_string()
        } else {
            format!("Bearer {}", b)
        }
    });

    let mut cfg = Configuration::default();
    cfg.base_path = api_base;

    let request = AgentMessageRequest::new(prompt_text);

    let rendered = tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            send_agent_message(&cfg, &api_key, bearer_header.as_deref(), request).await
        })?;
    Ok(rendered)
}

async fn send_agent_message(
    cfg: &Configuration,
    api_key: &str,
    bearer_opt: Option<&str>,
    request: AgentMessageRequest,
) -> Result<String, String> {
    let url = format!("{}/agent/response", cfg.base_path);
    let client = reqwest::Client::new();
    let mut req = client
        .post(url)
        .header("x-api-key", api_key)
        .header(ACCEPT, "text/event-stream, application/json")
        .header(CONTENT_TYPE, "application/json");
    if let Some(b) = bearer_opt {
        req = req.header(AUTHORIZATION, b);
    }

    let resp = req
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("request failed: status {} body: {}", status, body));
    }

    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if ctype.starts_with("text/event-stream") {
        // Collect the full SSE stream and return raw text for display
        let text = resp.text().await.unwrap_or_default();
        return Ok(text);
    }

    let text = resp.text().await.unwrap_or_default();
    if ctype.contains("application/json") {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            return Ok(serde_json::to_string_pretty(&json).unwrap_or(text));
        }
    }
    Ok(text)
}

async fn send_agent_message_stream(
    cfg: &Configuration,
    api_key: &str,
    bearer_opt: Option<&str>,
    request: AgentMessageRequest,
    tx: mpsc::Sender<String>,
) -> Result<(), String> {
    let url = format!("{}/agent/response", cfg.base_path);
    let client = reqwest::Client::new();
    let mut req = client
        .post(url)
        .header("x-api-key", api_key)
        .header(ACCEPT, "text/event-stream, application/json")
        .header(CONTENT_TYPE, "application/json");
    if let Some(b) = bearer_opt {
        req = req.header(AUTHORIZATION, b);
    }

    let mut resp = req
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let _ = tx.send(format!(
            "request failed: status {} body: {}\n",
            status, body
        ));
        return Err("request failed".to_string());
    }

    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if ctype.starts_with("text/event-stream") {
        let mut buffer = String::new();
        let mut saw_reasoning = false;
        let mut inserted_gap = false;
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            // Normalize line endings
            let normalized = buffer.replace("\r\n", "\n");
            let mut parts = normalized
                .split("\n\n")
                .map(|s| s.to_string())
                .collect::<Vec<_>>();
            // If the buffer doesn't end with a full event (double newline), keep the last partial
            buffer = match parts.pop() {
                Some(tail) if !normalized.ends_with("\n\n") => tail,
                Some(_) => String::new(),
                None => String::new(),
            };
            for ev in parts {
                let mut event_name: Option<String> = None;
                let mut data_lines: Vec<String> = Vec::new();
                for line in ev.lines() {
                    if let Some(rest) = line.strip_prefix("event:") {
                        event_name = Some(rest.trim().to_string());
                    } else if let Some(rest) = line.strip_prefix("data:") {
                        data_lines.push(rest.trim_start().to_string());
                    }
                }
                let data_str = data_lines.join("\n");
                if let Some(name) = event_name {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data_str) {
                        if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                            if name.contains("response.reasoning_summary_text.delta") {
                                let colored = format!("\x1b[90m{}\x1b[0m", delta);
                                let _ = tx.send(colored);
                                saw_reasoning = true;
                            } else if name.contains("response.output_text.delta") {
                                if saw_reasoning && !inserted_gap {
                                    let _ = tx.send("\n\n".to_string());
                                    inserted_gap = true;
                                }
                                let _ = tx.send(delta.to_string());
                            }
                        }
                        // Suppress all non-delta JSON events (no raw JSON output)
                    }
                    // Suppress non-JSON or unexpected events silently as well
                }
            }
        }
        return Ok(());
    }

    let text = resp.text().await.unwrap_or_default();
    if ctype.contains("application/json") {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            let pretty = serde_json::to_string_pretty(&json).unwrap_or(text);
            let _ = tx.send(pretty);
            return Ok(());
        }
    }
    let _ = tx.send(text);
    Ok(())
}

fn stream_and_print(
    cfg: &Configuration,
    api_key: &str,
    bearer_opt: Option<&str>,
    message: String,
) -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<String>();
    let cfg_clone = cfg.clone();
    let api_key_clone = api_key.to_string();
    let bearer_clone = bearer_opt.map(|s| s.to_string());
    thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(err) => {
                let _ = tx.send(format!("runtime error: {}\n", err));
                return;
            }
        };
        let request = AgentMessageRequest::new(message);
        let _ = rt.block_on(async move {
            let _ = send_agent_message_stream(
                &cfg_clone,
                &api_key_clone,
                bearer_clone.as_deref(),
                request,
                tx,
            )
            .await;
        });
    });

    let mut stdout = io::stdout();
    while let Ok(chunk) = rx.recv() {
        print!("{}", chunk);
        let _ = stdout.flush();
    }
    Ok(())
}
