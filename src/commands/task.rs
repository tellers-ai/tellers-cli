use std::time::Duration;

use clap::{Args, Subcommand};
use serde::Deserialize;
use tellers_api_client::apis::accepts_api_key_api as api;
use tellers_api_client::apis::configuration::Configuration;
use tokio::time::sleep;

use crate::commands::api_config;
use crate::output;

#[derive(Args, Debug)]
pub struct TaskArgs {
    #[command(subcommand)]
    pub command: TaskCommand,
}

#[derive(Subcommand, Debug)]
pub enum TaskCommand {
    /// Fetch the current status of a user task.
    Get(GetArgs),
    /// Poll a user task until it completes or fails.
    Wait(WaitArgs),
    /// Cancel a running user task.
    Cancel(CancelArgs),
}

#[derive(Args, Debug)]
pub struct GetArgs {
    pub task_id: String,

    #[arg(long, env = "TELLERS_API_KEY")]
    pub api_key: Option<String>,

    #[arg(long, env = "TELLERS_AUTH_BEARER")]
    pub auth_bearer: Option<String>,
}

#[derive(Args, Debug)]
pub struct WaitArgs {
    pub task_id: String,

    #[arg(long, default_value_t = 2)]
    pub interval_secs: u64,

    #[arg(long, env = "TELLERS_API_KEY")]
    pub api_key: Option<String>,

    #[arg(long, env = "TELLERS_AUTH_BEARER")]
    pub auth_bearer: Option<String>,
}

#[derive(Args, Debug)]
pub struct CancelArgs {
    pub task_id: String,

    #[arg(long, env = "TELLERS_API_KEY")]
    pub api_key: Option<String>,

    #[arg(long, env = "TELLERS_AUTH_BEARER")]
    pub auth_bearer: Option<String>,
}

#[derive(Debug, Clone)]
pub enum UserTaskStatus {
    Pending { progress: Option<f64> },
    Complete { result: serde_json::Value },
    Failed { result: Option<serde_json::Value> },
}

#[derive(Deserialize)]
struct RawUserTaskResponse {
    status: String,
    #[serde(default)]
    progress: Option<f64>,
    #[serde(default)]
    result: Option<serde_json::Value>,
}

pub fn run(args: TaskArgs) -> Result<(), String> {
    match args.command {
        TaskCommand::Get(get_args) => run_get(get_args),
        TaskCommand::Wait(wait_args) => run_wait(wait_args),
        TaskCommand::Cancel(cancel_args) => run_cancel(cancel_args),
    }
}

fn run_get(args: GetArgs) -> Result<(), String> {
    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(args.api_key)?;
    let bearer = api_config::get_bearer_header(args.auth_bearer);

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            let status =
                fetch_user_task(&cfg, &args.task_id, &api_key, bearer.as_deref()).await?;
            println!("{}", serde_json::to_string_pretty(&status_to_json(status))
                .map_err(|e| format!("failed to encode task status: {}", e))?);
            Ok(())
        })
}

fn run_wait(args: WaitArgs) -> Result<(), String> {
    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(args.api_key)?;
    let bearer = api_config::get_bearer_header(args.auth_bearer);

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            output::info(format!(
                "Polling /users/tasks/{} every {}s...",
                args.task_id, args.interval_secs
            ));
            let result = wait_for_user_task(
                &cfg,
                &args.task_id,
                &api_key,
                bearer.as_deref(),
                Duration::from_secs(args.interval_secs),
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)
                .map_err(|e| format!("failed to encode task result: {}", e))?);
            Ok(())
        })
}

fn run_cancel(args: CancelArgs) -> Result<(), String> {
    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(args.api_key)?;
    let bearer = api_config::get_bearer_header(args.auth_bearer);

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            let resp = api::cancel_user_task_users_tasks_task_id_delete(
                &cfg,
                &args.task_id,
                Some(&api_key),
                bearer.as_deref(),
            )
            .await
            .map_err(|e| api_config::format_api_error(&e))?;

            println!("task_id: {}", resp.task_id);
            println!("previous_state: {:?}", resp.previous_state);
            println!("description: {}", resp.description);
            Ok(())
        })
}

pub async fn fetch_user_task(
    cfg: &Configuration,
    task_id: &str,
    api_key: &str,
    bearer: Option<&str>,
) -> Result<UserTaskStatus, String> {
    let uri = format!(
        "{}/users/tasks/{}",
        cfg.base_path.trim_end_matches('/'),
        urlencoding_encode(task_id)
    );
    let mut req = cfg.client.request(reqwest::Method::GET, &uri);
    if let Some(user_agent) = &cfg.user_agent {
        req = req.header(reqwest::header::USER_AGENT, user_agent.clone());
    }
    req = req.header("x-api-key", api_key);
    if let Some(bearer) = bearer {
        req = req.header("authorization", bearer);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("failed to fetch task: {}", e))?;
    let status_code = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("failed to read task response: {}", e))?;
    if !status_code.is_success() {
        return Err(format!(
            "failed to fetch task; http_status: {}; response: {}",
            status_code, body
        ));
    }

    let raw: RawUserTaskResponse = serde_json::from_str(&body)
        .map_err(|e| format!("failed to parse task response: {}; body: {}", e, body))?;

    parse_user_task_status(&raw)
}

pub async fn wait_for_user_task(
    cfg: &Configuration,
    task_id: &str,
    api_key: &str,
    bearer: Option<&str>,
    poll_interval: Duration,
) -> Result<serde_json::Value, String> {
    loop {
        match fetch_user_task(cfg, task_id, api_key, bearer).await? {
            UserTaskStatus::Pending { progress } => {
                if let Some(progress) = progress {
                    output::info(format!("task progress: {:.0}%", progress_to_percent(progress)));
                }
                sleep(poll_interval).await;
            }
            UserTaskStatus::Complete { result } => return Ok(result),
            UserTaskStatus::Failed { result } => {
                let detail = result
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "unknown error".to_string());
                return Err(format!("task failed: {}", detail));
            }
        }
    }
}

fn parse_user_task_status(raw: &RawUserTaskResponse) -> Result<UserTaskStatus, String> {
    match raw.status.as_str() {
        "PENDING" => Ok(UserTaskStatus::Pending {
            progress: raw.progress,
        }),
        "COMPLETE" => Ok(UserTaskStatus::Complete {
            result: raw.result.clone().unwrap_or(serde_json::Value::Null),
        }),
        "FAILED" => Ok(UserTaskStatus::Failed {
            result: raw.result.clone(),
        }),
        other => Err(format!("unknown task status: {}", other)),
    }
}

fn status_to_json(status: UserTaskStatus) -> serde_json::Value {
    match status {
        UserTaskStatus::Pending { progress } => serde_json::json!({
            "status": "PENDING",
            "progress": progress,
        }),
        UserTaskStatus::Complete { result } => serde_json::json!({
            "status": "COMPLETE",
            "result": result,
        }),
        UserTaskStatus::Failed { result } => serde_json::json!({
            "status": "FAILED",
            "result": result,
        }),
    }
}

fn progress_to_percent(progress: f64) -> f64 {
    if (0.0..=1.0).contains(&progress) {
        progress * 100.0
    } else {
        progress.clamp(0.0, 100.0)
    }
}

fn urlencoding_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
