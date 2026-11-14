use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Args;
use tellers_api_client::apis::accepts_api_key_api as api;
use tellers_api_client::apis::configuration::Configuration;
use tellers_api_client::models::asset_download_url_batch_request::Quality;

use crate::commands::{api_config, asset, task};
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

    /// Wait for the export, then download the MP4 (implies --wait).
    #[arg(long, short = 'd', default_value_t = false)]
    pub download: bool,

    /// Where to save the downloaded MP4 (implies --download).
    /// Defaults to <PROJECT_ID>.mp4 in the current directory.
    #[arg(long, short = 'o', value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Replace an existing output file.
    #[arg(long, default_value_t = false)]
    pub force: bool,
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

/// `["720p,1080p", "4k"]` → validated `["720p", "1080p", "4k"]`; empty → `["1080p"]`.
pub fn parse_renditions(raw: &[String]) -> Result<Vec<String>, String> {
    let parts: Vec<String> = raw
        .iter()
        .flat_map(|s| s.split(',').map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return Ok(vec!["1080p".to_string()]);
    }
    parts.iter().map(|s| parse_rendition(s)).collect()
}

/// The started export: the task rendering it, and the asset it renders into.
pub struct StartedExport {
    pub task_id: String,
    pub asset_id: String,
}

pub async fn start_export(
    cfg: &Configuration,
    api_key: &str,
    bearer: Option<&str>,
    project_id: &str,
    renditions: Vec<String>,
) -> Result<StartedExport, String> {
    let resp = api::export_project_project_project_id_export_post(
        cfg,
        project_id,
        renditions,
        Some(api_key),
        bearer,
    )
    .await
    .map_err(|e| api_config::format_api_error(&e))?;
    Ok(StartedExport {
        task_id: resp.task_id,
        asset_id: resp.asset_id,
    })
}

/// Exports the project, waits for the render, and downloads the MP4 to
/// `destination`. Returns the exported asset ID.
pub async fn export_and_download(
    cfg: &Configuration,
    api_key: &str,
    bearer: Option<&str>,
    project_id: &str,
    renditions: Vec<String>,
    destination: &Path,
    force: bool,
) -> Result<String, String> {
    if destination.exists() && !force {
        return Err(format!(
            "destination already exists: {}; use --force to replace it",
            destination.display()
        ));
    }
    let started = start_export(cfg, api_key, bearer, project_id, renditions).await?;
    output::info(format!(
        "Exporting project {} (task {}, asset {})...",
        project_id, started.task_id, started.asset_id
    ));
    task::wait_for_user_task(cfg, &started.task_id, api_key, bearer, Duration::from_secs(2))
        .await?;
    output::info(format!("Downloading export to {}...", destination.display()));
    asset::download_asset(
        cfg,
        api_key,
        bearer,
        &started.asset_id,
        Quality::Highest,
        destination,
        force,
    )
    .await?;
    Ok(started.asset_id)
}

pub fn run(args: ExportArgs) -> Result<(), String> {
    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(args.api_key)?;
    let bearer_header = api_config::get_bearer_header(args.auth_bearer);
    let renditions = parse_renditions(&args.renditions)?;
    let download = args.download || args.output.is_some();

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            if download {
                let destination = args
                    .output
                    .unwrap_or_else(|| PathBuf::from(format!("{}.mp4", args.project_id)));
                export_and_download(
                    &cfg,
                    &api_key,
                    bearer_header.as_deref(),
                    &args.project_id,
                    renditions,
                    &destination,
                    args.force,
                )
                .await?;
                return Ok(());
            }

            let started = start_export(
                &cfg,
                &api_key,
                bearer_header.as_deref(),
                &args.project_id,
                renditions,
            )
            .await?;
            println!("task_id: {}", started.task_id);
            println!("asset_id: {}", started.asset_id);

            if args.wait {
                output::info(format!(
                    "Waiting for export task {} to complete...",
                    started.task_id
                ));
                let result = task::wait_for_user_task(
                    &cfg,
                    &started.task_id,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renditions_default_to_1080p() {
        assert_eq!(parse_renditions(&[]).unwrap(), vec!["1080p"]);
    }

    #[test]
    fn renditions_split_on_commas_and_validate() {
        let raw = vec!["720P, 1080p".to_string(), "4k".to_string()];
        assert_eq!(parse_renditions(&raw).unwrap(), vec!["720p", "1080p", "4k"]);
        assert!(parse_renditions(&["8k".to_string()]).is_err());
    }
}
