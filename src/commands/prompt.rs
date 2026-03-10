use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use std::io::{self, Write};
use std::sync::mpsc;
use std::thread;
use tellers_api_client::apis::configuration::Configuration;
use tellers_api_client::models::agent_message_request::LlmModel;
use tellers_api_client::models::agent_message_request_without_no_interaction::LlmModel as LlmModelJson;
use tellers_api_client::models::{AgentMessageRequest, AgentMessageRequestWithoutNoInteraction};

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

fn build_agent_request(message: String, opts: &PromptOptions) -> AgentMessageRequest {
    let mut req = AgentMessageRequest::new(message);
    req.no_interaction = Some(opts.no_interaction);
    if let Some(ref tools) = opts.tools {
        req.tools = Some(tools.clone());
    }
    if let Some(ref model) = opts.llm_model {
        if let Some(lm) = parse_llm_model(model) {
            req.llm_model = Some(lm);
        }
    }
    req
}

fn parse_llm_model_json(s: &str) -> Option<LlmModelJson> {
    serde_json::from_str::<LlmModelJson>(&format!("\"{}\"", s)).ok()
}

fn build_agent_request_json(
    message: String,
    opts: &PromptOptions,
) -> AgentMessageRequestWithoutNoInteraction {
    let mut req = AgentMessageRequestWithoutNoInteraction::new(message);
    if let Some(ref tools) = opts.tools {
        req.tools = Some(tools.clone());
    }
    if let Some(ref model) = opts.llm_model {
        if let Some(lm) = parse_llm_model_json(model) {
            req.llm_model = Some(lm);
        }
    }
    req
}

pub fn run_interactive(
    prompt_text: String,
    _full_auto: bool,
    opts: PromptOptions,
) -> Result<(), String> {
    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(None)?;
    let bearer_header = api_config::get_bearer_header(None);

    let opts_clone = opts.clone();
    stream_and_print(
        &cfg,
        &api_key,
        bearer_header.as_deref(),
        prompt_text.clone(),
        &opts_clone,
    )?;

    if opts.no_interaction || opts.json_response {
        return Ok(());
    }

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
        let opts_loop = opts.clone();
        stream_and_print(&cfg, &api_key, bearer_header.as_deref(), message, &opts_loop)?;
    }
    Ok(())
}

pub fn run_background(
    prompt_text: String,
    _full_auto: bool,
    opts: PromptOptions,
) -> Result<String, String> {
    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(None)?;
    let bearer_header = api_config::get_bearer_header(None);

    let request = if opts.json_response {
        let req = build_agent_request_json(prompt_text, &opts);
        tokio::runtime::Runtime::new()
            .map_err(|e| format!("failed to start runtime: {}", e))?
            .block_on(async move {
                send_agent_message_json(&cfg, &api_key, bearer_header.as_deref(), req).await
            })?
    } else {
        let request = build_agent_request(prompt_text, &opts);
        tokio::runtime::Runtime::new()
            .map_err(|e| format!("failed to start runtime: {}", e))?
            .block_on(async move {
                send_agent_message(&cfg, &api_key, bearer_header.as_deref(), request).await
            })?
    };
    Ok(request)
}

async fn send_agent_message_json(
    cfg: &Configuration,
    api_key: &str,
    bearer_opt: Option<&str>,
    request: AgentMessageRequestWithoutNoInteraction,
) -> Result<String, String> {
    let url = format!("{}/agent/response/json", cfg.base_path);
    let client = reqwest::Client::new();
    let mut req = client
        .post(&url)
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
    let text = resp.text().await.unwrap_or_default();
    if ctype.starts_with("text/event-stream") {
        let last = parse_last_json_result_from_sse(&text);
        return Ok(last.unwrap_or(text));
    }
    if ctype.contains("application/json") {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            return Ok(serde_json::to_string_pretty(&json).unwrap_or(text));
        }
    }
    Ok(text)
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
        .post(&url)
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
            let normalized = buffer.replace("\r\n", "\n");
            let mut parts = normalized
                .split("\n\n")
                .map(|s| s.to_string())
                .collect::<Vec<_>>();
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
                    }
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

fn parse_last_json_result_from_sse(sse_text: &str) -> Option<String> {
    let normalized = sse_text.replace("\r\n", "\n");
    let mut last_data: Option<String> = None;
    for ev in normalized.split("\n\n") {
        let mut event_name: Option<String> = None;
        let mut data_lines: Vec<&str> = Vec::new();
        for line in ev.lines() {
            if let Some(rest) = line.strip_prefix("event:") {
                event_name = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.trim_start());
            }
        }
        let data_str = data_lines.join("\n");
        if event_name.as_deref().map_or(false, |n| n.contains("tellers.json_result")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data_str) {
                last_data = Some(serde_json::to_string_pretty(&v).unwrap_or(data_str));
            }
        }
    }
    last_data
}

async fn send_agent_message_stream_json(
    cfg: &Configuration,
    api_key: &str,
    bearer_opt: Option<&str>,
    request: AgentMessageRequestWithoutNoInteraction,
    tx: mpsc::Sender<String>,
) -> Result<(), String> {
    let url = format!("{}/agent/response/json", cfg.base_path);
    let client = reqwest::Client::new();
    let mut req = client
        .post(&url)
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
        let mut last_json_result: Option<String> = None;
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            let normalized = buffer.replace("\r\n", "\n");
            let mut parts = normalized
                .split("\n\n")
                .map(|s| s.to_string())
                .collect::<Vec<_>>();
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
                if let Some(name) = &event_name {
                    if name.contains("tellers.json_result") {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data_str) {
                            let pretty = serde_json::to_string_pretty(&v).unwrap_or(data_str);
                            last_json_result = Some(pretty);
                        }
                    }
                }
            }
        }
        if let Some(pretty) = last_json_result {
            let _ = tx.send(pretty);
            let _ = tx.send("\n".to_string());
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
    opts: &PromptOptions,
) -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<String>();
    let cfg_clone = cfg.clone();
    let api_key_clone = api_key.to_string();
    let bearer_clone = bearer_opt.map(|s| s.to_string());
    let opts_clone = opts.clone();

    if opts.json_response {
        let request = build_agent_request_json(message, opts);
        thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(err) => {
                    let _ = tx.send(format!("runtime error: {}\n", err));
                    return;
                }
            };
            rt.block_on(async move {
                let _ = send_agent_message_stream_json(
                    &cfg_clone,
                    &api_key_clone,
                    bearer_clone.as_deref(),
                    request,
                    tx,
                )
                .await;
            });
        });
    } else {
        let request = build_agent_request(message, &opts_clone);
        thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(err) => {
                    let _ = tx.send(format!("runtime error: {}\n", err));
                    return;
                }
            };
            rt.block_on(async move {
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
    }

    let mut stdout = io::stdout();
    while let Ok(chunk) = rx.recv() {
        print!("{}", chunk);
        let _ = stdout.flush();
    }
    Ok(())
}
