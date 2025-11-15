use clap::Args;
use tellers_api_client::apis::accepts_api_key_api as api;

use crate::commands::api_config;
use crate::output;

#[derive(Args, Debug)]
pub struct ListArgs {
    #[arg(long, env = "TELLERS_API_KEY")]
    pub api_key: Option<String>,

    #[arg(long, env = "TELLERS_AUTH_BEARER")]
    pub auth_bearer: Option<String>,
}

pub fn run(args: ListArgs) -> Result<(), String> {
    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(args.api_key)?;
    let bearer_header = api_config::get_bearer_header(args.auth_bearer);

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            let groups = api::list_groups_group_list_get(
                &cfg,
                Some(&api_key),
                bearer_header.as_deref(),
            )
            .await
            .map_err(|e| {
                let mut m = format!("failed to list groups: {}", e);
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

            output::info(format!("Retrieved {} group(s)", groups.len()));

            if groups.is_empty() {
                output::info("No groups found");
            } else {
                println!("{}", serde_json::to_string_pretty(&groups).unwrap_or_default());
            }

            Ok(())
        })
}

