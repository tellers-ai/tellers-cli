use std::time::Duration;

use clap::Args;
use tellers_api_client::apis::accepts_api_key_api as api;

use crate::commands::{api_config, task};
use crate::output;

#[derive(Args, Debug)]
pub struct ExportArgs {
    /// Project ID to export
    #[arg(value_name = "PROJECT_ID")]
    pub project_id: String,

    /// Renditions to export (360p, 480p, 720p, 1080p, 1440p, 4k). Default 1080p when omitted.
    #[arg(long = "rendition", short = 'r', value_name = "RESOLUTION")]
    pub renditions: Vec<String>,

    #[arg(long, env = "TELLERS_API_KEY")]
    pub api_key: Option<String>,

    #[arg(long, env = "TELLERS_AUTH_BEARER")]
    pub auth_bearer: Option<String>,

    /// Poll GET /users/tasks/{task_id} until the export completes.
    #[arg(long, default_value_t = false)]
    pub wait: bool,
}

const ALLOWED_RENDITIONS: &[&str] = &["360p", "480p", "720p", "1080p", "1440p", "4k"];

fn parse_rendition(s: &str) -> Result<String, String> {
    let v = s.trim().to_lowercase();
    if ALLOWED_RENDITIONS.contains(&v.as_str()) {
        Ok(v)
    } else {
        Err(format!(
            "Invalid rendition '{}'; allowed: {}",
            s,
            ALLOWED_RENDITIONS.join(", ")
        ))
    }
}

pub fn run(args: ExportArgs) -> Result<(), String> {
    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(args.api_key)?;
    let bearer_header = api_config::get_bearer_header(args.auth_bearer);

    let rendition_strs: Vec<String> = if args.renditions.is_empty() {
        vec!["1080p".to_string()]
    } else {
        args.renditions
            .iter()
            .flat_map(|s| s.split(',').map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect()
    };

    let renditions: Vec<String> = rendition_strs
        .iter()
        .map(|s| parse_rendition(s))
        .collect::<Result<Vec<_>, _>>()?;

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            let resp = api::export_project_project_project_id_export_post(
                &cfg,
                &args.project_id,
                renditions,
                Some(&api_key),
                bearer_header.as_deref(),
            )
            .await
            .map_err(|e| api_config::format_api_error(&e))?;

            println!("task_id: {}", resp.task_id);
            println!("asset_id: {}", resp.asset_id);

            if args.wait {
                output::info(format!(
                    "Waiting for export task {} to complete...",
                    resp.task_id
                ));
                let result = task::wait_for_user_task(
                    &cfg,
                    &resp.task_id,
                    &api_key,
                    bearer_header.as_deref(),
                    Duration::from_secs(2),
                )
                .await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result)
                        .map_err(|e| format!("failed to encode export result: {}", e))?
                );
            }

            Ok(())
        })
}
