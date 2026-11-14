use reqwest::header::AUTHORIZATION;
use serde_json::Value;
use std::io::{self, Write};
use std::path::PathBuf;
use tellers_api_client::apis::configuration::Configuration;
use tellers_api_client::models::agent_message_request::LlmModel;
use tellers_api_client::models::asset_download_url_batch_request::Quality;
use tellers_api_client::models::AgentMessageRequest;

use crate::commands::agent_ws::{self, Auth, Sent};
use crate::commands::{api_config, asset, project};
use crate::output;

#[derive(Clone, Default, Debug)]
pub struct PromptOptions {
    pub no_interaction: bool,
    pub json_response: bool,
    pub tools: Option<Vec<String>>,
    pub llm_model: Option<String>,
    pub export: Option<ExportOptions>,
}

/// `--export-to`: the prompt asks the agent to export what it made and name
/// it on an [`EXPORT_MARKER`] line; once the turn ends, each export is
/// downloaded into `dir` as `<project name>.mp4`.
#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub dir: PathBuf,
    pub renditions: Vec<String>,
    pub force: bool,
    /// Print the appended instructions before sending (`--show-instructions`).
    pub show_instructions: bool,
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
            export: None,
        }
    }

    pub fn with_export(
        mut self,
        dir: Option<PathBuf>,
        renditions: &[String],
        force: bool,
        show_instructions: bool,
    ) -> Result<Self, String> {
        if let Some(dir) = dir {
            self.export = Some(ExportOptions {
                dir,
                renditions: project::parse_renditions(renditions)?,
                force,
                show_instructions,
            });
        }
        Ok(self)
    }

    /// The turn runs to the end on its own, with no question back.
    fn no_interaction(&self) -> bool {
        self.no_interaction || self.json_response || self.export.is_some()
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

/// The request body for `POST /agent/messages`. JSON and export modes run the
/// turn to the end on its own (`no_interaction`); export mode also appends
/// [`export_instructions`] to the message.
fn build_agent_request(
    message: String,
    opts: &PromptOptions,
    chat_id: Option<&str>,
) -> Result<Value, String> {
    let message = match opts.export {
        Some(ref export) => {
            let instructions = export_instructions(&export.renditions);
            if export.show_instructions {
                show_instructions(&instructions);
            }
            format!("{}{}", message, instructions)
        }
        None => message,
    };
    let mut req = AgentMessageRequest::new(message);
    req.no_interaction = Some(opts.no_interaction());
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

/// Starts the line on which the agent reports each export, followed by one
/// JSON object: `{"project_id", "project_name", "asset_id", "completed"}`.
pub const EXPORT_MARKER: &str = "TELLERS_EXPORT";

/// Appended to the user's prompt with `--export-to`: do the whole job without
/// asking, export the result, and report it on a line the CLI can parse.
fn export_instructions(renditions: &[String]) -> String {
    format!(
        "\n\n---\n\
Instructions from the Tellers CLI (the user is not watching and cannot answer):\n\
1. Do the whole request above on your own, without asking any question. \
When something is unclear, make a reasonable choice and go on. \
Build the result in a Tellers project (create one if needed) and give it a short, descriptive name.\n\
2. When the edit is finished, export each project you built or changed to MP4 with \
the export tool, at {renditions}, with wait_for_completion set to true and notify_user set to true.\n\
3. End your final message with one line per exported project, exactly in this form, \
on its own line, with nothing else on that line and no code block around it:\n\
{marker} {{\"project_id\": \"<project uuid>\", \"project_name\": \"<project name>\", \
\"asset_id\": \"<export asset uuid>\", \"completed\": <true or false>}}\n\
Use the export asset ID the export tool returned, and its completed value. \
If the export failed or you could not export, still write the line, with \"asset_id\": null \
and \"completed\": false.\n",
        renditions = renditions.join(", "),
        marker = EXPORT_MARKER,
    )
}

/// Prints what the CLI adds to the user's prompt, in grey on stderr (so a
/// `--background` answer on stdout stays clean).
fn show_instructions(instructions: &str) {
    eprintln!(
        "\x1b[90m[added to your prompt by --export-to]{}\x1b[0m\n",
        instructions.trim_end()
    );
}

/// One export the agent reported on an [`EXPORT_MARKER`] line.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct ReportedExport {
    pub project_id: String,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub asset_id: Option<String>,
    #[serde(default)]
    pub completed: bool,
}

/// Every [`EXPORT_MARKER`] line of the agent's answer, in order, one per
/// project (the last line wins). Tolerates markdown around the line
/// (backticks, bullets, bold) and a `:` after the marker.
fn parse_export_lines(answer: &str) -> Vec<ReportedExport> {
    let mut exports: Vec<ReportedExport> = Vec::new();
    for line in answer.lines() {
        let Some(at) = line.find(EXPORT_MARKER) else {
            continue;
        };
        let rest = line[at + EXPORT_MARKER.len()..].trim_start_matches(|c: char| c == ':' || c.is_whitespace());
        let (Some(open), Some(close)) = (rest.find('{'), rest.rfind('}')) else {
            continue;
        };
        if close < open {
            continue;
        }
        let Ok(export) = serde_json::from_str::<ReportedExport>(&rest[open..=close]) else {
            continue;
        };
        if export.project_id.trim().is_empty() {
            continue;
        }
        match exports.iter_mut().find(|e| e.project_id == export.project_id) {
            Some(seen) => *seen = export,
            None => exports.push(export),
        }
    }
    exports
}

/// A project name made safe as a file name (path separators, reserved and
/// control characters replaced), or `None` when nothing usable is left.
fn file_stem(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(150).collect())
}

/// `<dir>/<project name>.mp4`; without `force`, a free `<name> (2).mp4`, …
/// when that file exists already. Falls back to the project ID as the name.
fn export_destination(dir: &std::path::Path, export: &ReportedExport, force: bool) -> PathBuf {
    let stem = export
        .project_name
        .as_deref()
        .and_then(file_stem)
        .unwrap_or_else(|| export.project_id.clone());
    let first = dir.join(format!("{}.mp4", stem));
    if force || !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{} ({}).mp4", stem, n)))
        .find(|path| !path.exists())
        .expect("an unused file name")
}

/// Downloads each export the agent reported into `export.dir`. An export the
/// agent did not finish (no asset, or not completed) is redone by the CLI.
async fn download_reported_exports(
    cfg: &Configuration,
    auth: &Auth<'_>,
    answer: &str,
    export: &ExportOptions,
) -> Result<Vec<PathBuf>, String> {
    let reported = parse_export_lines(answer);
    if reported.is_empty() {
        return Err(format!(
            "the agent's answer has no {} line: nothing to download",
            EXPORT_MARKER
        ));
    }
    std::fs::create_dir_all(&export.dir)
        .map_err(|e| format!("failed to create {}: {}", export.dir.display(), e))?;
    let mut files = Vec::with_capacity(reported.len());
    for item in reported {
        let destination = export_destination(&export.dir, &item, export.force);
        match item.asset_id.as_deref().filter(|_| item.completed) {
            Some(asset_id) => {
                output::info(format!("Downloading export {} to {}...", asset_id, destination.display()));
                asset::download_asset(
                    cfg,
                    auth.api_key,
                    auth.bearer,
                    asset_id,
                    Quality::Highest,
                    &destination,
                    export.force,
                )
                .await?;
            }
            None => {
                output::warning(format!(
                    "The agent did not finish exporting project {}; exporting it now",
                    item.project_id
                ));
                project::export_and_download(
                    cfg,
                    auth.api_key,
                    auth.bearer,
                    &item.project_id,
                    export.renditions.clone(),
                    &destination,
                    export.force,
                )
                .await?;
            }
        }
        output::success(format!("Saved {}", destination.display()));
        files.push(destination);
    }
    Ok(files)
}

/// Sends one message and prints the turn that takes it (or, in JSON mode,
/// the turn's parsed result); with `--export-to`, then downloads what the
/// agent exported. Returns the chat id, for the next message.
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
    let mut answer = String::new();
    agent_ws::follow_turn(cfg, auth, &sent, |event, data| {
        printer.on_event(event, data);
        if event == "response.output_text.delta" {
            if let Some(delta) = data.get("delta").and_then(Value::as_str) {
                answer.push_str(delta);
            }
        }
    })
    .await?;
    println!();
    if let Some(ref export) = opts.export {
        download_reported_exports(cfg, auth, &answer, export).await?;
    }
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
    if opts.no_interaction() {
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
        if let (Some(message), true) = (&error, answer.is_empty()) {
            return Err(message.clone());
        }
        if let Some(ref export) = opts.export {
            let files = download_reported_exports(&cfg, &auth, &answer, export).await?;
            let paths: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
            return Ok(format!("{}\n{}", answer.trim_end(), paths.join("\n")));
        }
        Ok(answer)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const P1: &str = "11111111-1111-1111-1111-111111111111";
    const P2: &str = "22222222-2222-2222-2222-222222222222";

    #[test]
    fn export_lines_are_parsed_through_markdown() {
        let answer = format!(
            "Done!\n\
             - `TELLERS_EXPORT {{\"project_id\": \"{P1}\", \"project_name\": \"Cats\", \"asset_id\": \"a1\", \"completed\": true}}`\n\
             **TELLERS_EXPORT:** {{\"project_id\": \"{P2}\", \"asset_id\": null, \"completed\": false}}\n\
             TELLERS_EXPORT not json\n"
        );
        let parsed = parse_export_lines(&answer);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].project_name.as_deref(), Some("Cats"));
        assert_eq!(parsed[0].asset_id.as_deref(), Some("a1"));
        assert!(parsed[0].completed);
        assert_eq!(parsed[1].project_id, P2);
        assert!(parsed[1].asset_id.is_none() && !parsed[1].completed);
    }

    #[test]
    fn last_export_line_of_a_project_wins() {
        let answer = format!(
            "TELLERS_EXPORT {{\"project_id\": \"{P1}\", \"completed\": false}}\n\
             TELLERS_EXPORT {{\"project_id\": \"{P1}\", \"asset_id\": \"a2\", \"completed\": true}}"
        );
        let parsed = parse_export_lines(&answer);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].asset_id.as_deref(), Some("a2"));
    }

    #[test]
    fn project_names_become_safe_file_names() {
        assert_eq!(file_stem("Cats / Dogs: v2?").as_deref(), Some("Cats _ Dogs_ v2_"));
        assert_eq!(file_stem("  ..  "), None);
        assert_eq!(file_stem("../etc").as_deref(), Some("_etc"));
    }

    #[test]
    fn destination_uses_the_name_and_avoids_overwrites() {
        let dir = std::env::temp_dir().join(format!("tellers-cli-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let named = ReportedExport {
            project_id: P1.into(),
            project_name: Some("My cats".into()),
            asset_id: None,
            completed: false,
        };
        assert_eq!(export_destination(&dir, &named, false), dir.join("My cats.mp4"));
        std::fs::write(dir.join("My cats.mp4"), b"x").unwrap();
        assert_eq!(export_destination(&dir, &named, false), dir.join("My cats (2).mp4"));
        assert_eq!(export_destination(&dir, &named, true), dir.join("My cats.mp4"));
        let unnamed = ReportedExport { project_name: None, ..named };
        assert_eq!(export_destination(&dir, &unnamed, false), dir.join(format!("{P1}.mp4")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn export_mode_appends_instructions_and_runs_without_interaction() {
        let opts = PromptOptions::default()
            .with_export(Some(PathBuf::from("out")), &["720p".to_string()], false, false)
            .unwrap();
        let body = build_agent_request("make a cat video".into(), &opts, None).unwrap();
        assert_eq!(body["no_interaction"], json!(true));
        let message = body["message"].as_str().unwrap();
        assert!(message.starts_with("make a cat video"));
        assert!(message.contains(EXPORT_MARKER));
        assert!(message.contains("720p"));
        assert!(message.contains("wait_for_completion"));
    }
}
