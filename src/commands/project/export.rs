use clap::Args;
use tellers_api_client::apis::accepts_api_key_api as api;

use crate::commands::api_config;

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
            .map_err(|e| {
                let mut m = format!("export failed: {}", e);
                match &e {
                    tellers_api_client::apis::Error::Reqwest(req_err) => {
                        if let Some(status) = req_err.status() {
                            m.push_str(&format!("; http_status: {}", status));
                        }
                    }
                    tellers_api_client::apis::Error::ResponseError(resp) => {
                        m.push_str(&format!("; http_status: {}", resp.status));
                        if !resp.content.is_empty() {
                            m.push_str(&format!("; response: {}", resp.content));
                        }
                    }
                    _ => {}
                }
                m
            })?;

            println!("task_id: {}", resp.task_id);
            println!("asset_id: {}", resp.asset_id);
            Ok(())
        })
}
