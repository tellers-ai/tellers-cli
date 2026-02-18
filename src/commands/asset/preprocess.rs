use clap::Args;
use tellers_api_client::apis::accepts_api_key_api as api;
use tellers_api_client::models::{AssetUploadResponse, ProcessAssetsRequest};

use crate::commands::api_config;

#[derive(Args, Debug)]
pub struct PreprocessArgs {
    #[arg(required = true, num_args = 1..)]
    pub ids: Vec<String>,

    #[arg(long, default_value_t = false)]
    pub disable_description_generation: bool,

    #[arg(long, env = "TELLERS_API_KEY")]
    pub api_key: Option<String>,

    #[arg(long, env = "TELLERS_AUTH_BEARER")]
    pub auth_bearer: Option<String>,
}

pub fn run(args: PreprocessArgs) -> Result<(), String> {
    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(args.api_key)?;
    let bearer_header = api_config::get_bearer_header(args.auth_bearer);

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            let assets: Vec<AssetUploadResponse> = args
                .ids
                .into_iter()
                .map(|asset_id| AssetUploadResponse::new(String::new(), String::new(), asset_id))
                .collect();

            let mut preproc_req = ProcessAssetsRequest::new(
                assets.clone(),
                None::<tellers_api_client::models::VersionReference>,
            );
            preproc_req.generate_time_based_media_description =
                Some(!args.disable_description_generation);

            println!("Triggering preprocessing for {} asset(s)...", preproc_req.assets.len());

            let preproc_tasks = api::process_assets_users_assets_preprocess_post(
                &cfg,
                preproc_req,
                None,
                Some(&api_key),
                bearer_header.as_deref(),
            )
            .await
            .map_err(|e| {
                let mut m = format!("failed to trigger preprocess: {}", e);
                match &e {
                    tellers_api_client::apis::Error::Reqwest(req_err) => {
                        if let Some(status) = req_err.status() {
                            m.push_str(&format!("; http_status: {}", status));
                        }
                    }
                    tellers_api_client::apis::Error::ResponseError(resp) => {
                        m.push_str(&format!("; http_status: {}", resp.status));
                        if !resp.content.is_empty() {
                            m.push_str(&format!("; response_body: {}", resp.content));
                        }
                    }
                    _ => {}
                }
                m
            })?;

            println!("Preprocess tasks queued: {}", preproc_tasks.len());
            for task in preproc_tasks {
                if let Some(Some(ref error_msg)) = task.error {
                    eprintln!("Task {} error: {}", task.task_id, error_msg);
                } else {
                    println!("Task {} queued successfully", task.task_id);
                }
            }

            Ok(())
        })
}

