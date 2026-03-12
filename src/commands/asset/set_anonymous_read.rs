use clap::Args;
use tellers_api_client::models::AssetVisibilityRequest;

use crate::commands::api_config;

#[derive(Args, Debug)]
pub struct SetAnonymousReadArgs {
    /// Asset ID to set anonymous read for
    pub asset_id: String,

    /// Set anonymous read to true or false (default: true).
    #[arg(long, default_value_t = true, value_parser = clap::value_parser!(bool))]
    pub allow: bool,

    #[arg(long, env = "TELLERS_API_KEY", hide = true)]
    pub api_key: Option<String>,

    #[arg(long, env = "TELLERS_AUTH_BEARER", hide = true)]
    pub auth_bearer: Option<String>,
}

pub fn run(args: SetAnonymousReadArgs) -> Result<(), String> {
    let base = api_config::get_api_base();
    let api_key = api_config::get_api_key(args.api_key)?;
    let bearer = api_config::get_bearer_header(args.auth_bearer);

    let url = format!(
        "{}/asset/{}/visibility",
        base.trim_end_matches('/'),
        args.asset_id
    );
    let body = AssetVisibilityRequest::new(args.allow);

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            let client = reqwest::Client::new();
            let mut req = client
                .put(&url)
                .json(&body)
                .header("x-api-key", &api_key);
            if let Some(ref b) = bearer {
                req = req.header("authorization", b);
            }
            let resp = req.send().await.map_err(|e| format!("request failed: {}", e))?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(format!(
                    "set_anonymous_read failed; http_status: {}; response: {}",
                    status, text
                ));
            }
            println!(
                "anonymous_read set to {} for asset {}",
                args.allow, args.asset_id
            );
            Ok(())
        })
}
