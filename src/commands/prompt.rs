use reqwest::header::AUTHORIZATION;
use serde_json::Value;
use std::io::{self, Write};
use tellers_api_client::apis::configuration::Configuration;
use tellers_api_client::models::agent_message_request::LlmModel;
use tellers_api_client::models::AgentMessageRequest;

use crate::commands::agent_ws::{self, Auth, Sent};
use crate::commands::api_config;

#[derive(Clone, Default, Debug)]
pub struct PromptOptions {
    pub no_interaction: bool,
    pub json_response: bool,
    pub tools: Option<Vec<String>>,
    pub llm_model: Option<String>,
}

impl PromptOptions {
    pub fn from_cli(
        no_interaction: bool,
        json_response: bool,
        tools: Vec<String>,
        llm_model: Option<String>,
    ) -> Self {
        Self {
            no_interaction,
            json_response,
            tools: if tools.is_empty() { None } else { Some(tools) },
            llm_model,
        }
    }

}

pub fn run_interactive_options(
    base: &PromptOptions,
) -> Result<PromptOptions, String> {
    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(None)?;
    let bearer_header = api_config::get_bearer_header(None);

    let (models, tool_ids, tool_default_checked) = tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async {
            fetch_models_and_tool_ids(&cfg, &api_key, bearer_header.as_deref()).await
        })?;

    let mut opts = base.clone();
    let mut stdout = io::stdout();
    let stdin = io::stdin();

    print!("Use JSON response endpoint? [y/N]: ");
    let _ = stdout.flush();
    let mut line = String::new();
    let _ = stdin.read_line(&mut line);
    if line.trim().eq_ignore_ascii_case("y") || line.trim().eq_ignore_ascii_case("yes") {
        opts.json_response = true;
    }

    if !opts.json_response {
        print!("No interaction (single response, no REPL)? [y/N]: ");
        let _ = stdout.flush();
        line.clear();
        let _ = stdin.read_line(&mut line);
        if line.trim().eq_ignore_ascii_case("y") || line.trim().eq_ignore_ascii_case("yes") {
            opts.no_interaction = true;
        }
    }

    if !tool_ids.is_empty() {
        match crate::tui::run_checkbox_list(
            "Select tools (Space=toggle, Enter=confirm)",
            tool_ids,
            Some(tool_default_checked),
        ) {
            Ok(selected) => {
                if !selected.is_empty() {
                    opts.tools = Some(selected);
                }
            }
            Err(e) => return Err(e),
        }
    }

    if !models.is_empty() {
        println!("Available LLM models:");
        for (i, m) in models.iter().enumerate() {
            println!("  {}: {}", i + 1, m);
        }
        print!("Select model (number or Enter to use default): ");
        let _ = stdout.flush();
        line.clear();
        let _ = stdin.read_line(&mut line);
        let input = line.trim();
        if !input.is_empty() {
            if let Ok(n) = input.parse::<usize>() {
                if n >= 1 && n <= models.len() {
                    opts.llm_model = Some(models[n - 1].clone());
                }
            } else {
                opts.llm_model = Some(input.to_string());
            }
        }
    }

    Ok(opts)
}

async fn fetch_models_and_tool_ids(
    cfg: &Configuration,
    api_key: &str,
    bearer_opt: Option<&str>,
) -> Result<(Vec<String>, Vec<String>, Vec<bool>), String> {
    let settings = fetch_settings(cfg, api_key, bearer_opt).await?;
    let models: Vec<String> = settings
        .get("available_llm_models")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let (tool_ids, default_checked): (Vec<String>, Vec<bool>) = settings
        .get("available_agent_tools")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let mut ids = Vec::with_capacity(arr.len());
            let mut checked = Vec::with_capacity(arr.len());
            for o in arr.iter() {
                let obj = o.as_object()?;
                let id = obj
                    .get("id")
                    .or_else(|| obj.get("name"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                if let Some(id) = id {
                    let enabled = obj
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    ids.push(id);
                    checked.push(enabled);
                }
            }
            Some((ids, checked))
        })
        .and_then(|x| x)
        .unwrap_or_else(|| (Vec::new(), Vec::new()));
    Ok((models, tool_ids, default_checked))
}

async fn fetch_settings(
    cfg: &Configuration,
    api_key: &str,
    bearer_opt: Option<&str>,
) -> Result<serde_json::Value, String> {
    let url = format!("{}/settings", cfg.base_path);
    let client = reqwest::Client::new();
    let mut req = client.get(&url).header("x-api-key", api_key);
    if let Some(b) = bearer_opt {
        req = req.header(AUTHORIZATION, b);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("request failed: status {} body: {}", status, body));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("invalid response: {}", e))?;
    Ok(body)
}

fn parse_llm_model(s: &str) -> Option<LlmModel> {
    serde_json::from_str::<LlmModel>(&format!("\"{}\"", s)).ok()
}

/// The request body for `POST /agent/messages`. JSON mode runs the turn to
/// the end on its own (`no_interaction`), which is what its parsed result
/// (projects, assets, summary) is built from.
fn build_agent_request(
    message: String,
    opts: &PromptOptions,
    chat_id: Option<&str>,
) -> Result<Value, String> {
    let mut req = AgentMessageRequest::new(message);
    req.no_interaction = Some(opts.no_interaction || opts.json_response);
    if let Some(ref tools) = opts.tools {
        req.tools = Some(tools.clone());
    }
    if let Some(ref model) = opts.llm_model {
        if let Some(lm) = parse_llm_model(model) {
            req.llm_model = Some(lm);
        }
    }
    let mut body = serde_json::to_value(&req).map_err(|e| e.to_string())?;
    if let (Some(chat_id), Some(object)) = (chat_id, body.as_object_mut()) {
        object.insert("chat_id".to_string(), Value::String(chat_id.to_string()));
    }
    Ok(body)
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Runtime::new().map_err(|e| format!("failed to start runtime: {}", e))
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// Prints a turn as it streams: reasoning in grey, then the answer.
struct TurnPrinter {
    saw_reasoning: bool,
    inserted_gap: bool,
}

impl TurnPrinter {
    fn new() -> Self {
        Self {
            saw_reasoning: false,
            inserted_gap: false,
        }
    }

    fn on_event(&mut self, event: &str, data: &Value) {
        let mut stdout = io::stdout();
        match event {
            "response.reasoning_summary_text.delta" => {
                if let Some(delta) = data.get("delta").and_then(Value::as_str) {
                    print!("\x1b[90m{}\x1b[0m", delta);
                    self.saw_reasoning = true;
                }
            }
            "response.output_text.delta" => {
                if let Some(delta) = data.get("delta").and_then(Value::as_str) {
                    if self.saw_reasoning && !self.inserted_gap {
                        print!("\n\n");
                        self.inserted_gap = true;
                    }
                    print!("{}", delta);
                }
            }
            "tellers.agent.error" => {
                let message = data.get("message").and_then(Value::as_str).unwrap_or("error");
                eprintln!("\nerror: {}", message);
            }
            _ => {}
        }
        let _ = stdout.flush();
    }
}

/// Sends one message and prints the turn that takes it (or, in JSON mode,
/// the turn's parsed result). Returns the chat id, for the next message.
async fn send_and_print(
    cfg: &Configuration,
    auth: &Auth<'_>,
    message: String,
    opts: &PromptOptions,
    chat_id: Option<&str>,
) -> Result<String, String> {
    let body = build_agent_request(message, opts, chat_id)?;
    let sent = agent_ws::send_message(cfg, auth, &body).await?;
    if opts.json_response {
        let result = agent_ws::wait_for_result(cfg, auth, &sent.chat_id).await?;
        println!("{}", pretty(&result));
        return Ok(sent.chat_id);
    }
    let mut printer = TurnPrinter::new();
    agent_ws::follow_turn(cfg, auth, &sent, |event, data| printer.on_event(event, data)).await?;
    Ok(sent.chat_id)
}

pub fn run_interactive(
    prompt_text: String,
    _full_auto: bool,
    opts: PromptOptions,
) -> Result<(), String> {
    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(None)?;
    let bearer_header = api_config::get_bearer_header(None);
    let auth = Auth {
        api_key: &api_key,
        bearer: bearer_header.as_deref(),
    };
    let rt = runtime()?;

    let mut chat_id = rt.block_on(send_and_print(&cfg, &auth, prompt_text, &opts, None))?;
    if opts.no_interaction || opts.json_response {
        return Ok(());
    }

    // Every message of the session goes to the same chat, so the agent keeps
    // the conversation.
    let mut stdout = io::stdout();
    let stdin = io::stdin();
    loop {
        print!("\nYou: ");
        let _ = stdout.flush();
        let mut line = String::new();
        // 0 bytes is end of input (Ctrl-D, or a piped prompt run out): stop,
        // rather than reading empty lines forever.
        match stdin.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let message = line.trim().to_string();
        if message.is_empty() {
            continue;
        }
        chat_id = rt.block_on(send_and_print(&cfg, &auth, message, &opts, Some(&chat_id)))?;
    }
    Ok(())
}

/// One-shot: the turn's parsed result as JSON with `--json-response`,
/// otherwise the agent's answer as plain text.
pub fn run_background(
    prompt_text: String,
    _full_auto: bool,
    opts: PromptOptions,
) -> Result<String, String> {
    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(None)?;
    let bearer_header = api_config::get_bearer_header(None);
    let auth = Auth {
        api_key: &api_key,
        bearer: bearer_header.as_deref(),
    };
    runtime()?.block_on(async {
        let body = build_agent_request(prompt_text, &opts, None)?;
        let sent: Sent = agent_ws::send_message(&cfg, &auth, &body).await?;
        if opts.json_response {
            let result = agent_ws::wait_for_result(&cfg, &auth, &sent.chat_id).await?;
            return Ok(pretty(&result));
        }
        let mut answer = String::new();
        let mut error: Option<String> = None;
        agent_ws::follow_turn(&cfg, &auth, &sent, |event, data| match event {
            "response.output_text.delta" => {
                if let Some(delta) = data.get("delta").and_then(Value::as_str) {
                    answer.push_str(delta);
                }
            }
            "tellers.agent.error" => {
                error = data.get("message").and_then(Value::as_str).map(String::from);
            }
            _ => {}
        })
        .await?;
        match error {
            Some(message) if answer.is_empty() => Err(message),
            _ => Ok(answer),
        }
    })
}
