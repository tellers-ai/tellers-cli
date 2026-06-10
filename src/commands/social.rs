use clap::{Args, Subcommand};
use tellers_api_client::apis::accepts_api_key_api as api;
use tellers_api_client::models::SocialPublishPostRequest;

use crate::commands::api_config;

#[derive(Args, Debug)]
pub struct SocialArgs {
    #[command(subcommand)]
    pub command: SocialCommand,
}

#[derive(Subcommand, Debug)]
pub enum SocialCommand {
    /// Get the Upload-Post OAuth connect URL for linking social accounts.
    Connect(ConnectArgs),
    /// Publish an asset to one or more social platforms.
    Publish(PublishArgs),
}

#[derive(Args, Debug)]
pub struct ConnectArgs {
    /// Platform(s) to connect (repeat for multiple).
    #[arg(long = "platform", value_name = "PLATFORM")]
    pub platforms: Vec<String>,

    #[arg(long)]
    pub redirect_url: Option<String>,

    #[arg(long, default_value_t = false)]
    pub show_calendar: bool,

    #[arg(long, env = "TELLERS_AUTH_BEARER")]
    pub auth_bearer: Option<String>,
}

#[derive(Args, Debug)]
pub struct PublishArgs {
    /// Asset ID to publish.
    pub asset_id: String,

    /// Target platform(s) (repeat for multiple).
    #[arg(long = "platform", required = true, value_name = "PLATFORM")]
    pub platforms: Vec<String>,

    #[arg(long, default_value = "")]
    pub title: String,

    #[arg(long, default_value = "")]
    pub description: String,

    /// ISO 8601 datetime for scheduled publishing.
    #[arg(long)]
    pub scheduled_date: Option<String>,

    #[arg(long, env = "TELLERS_API_KEY")]
    pub api_key: Option<String>,

    #[arg(long, env = "TELLERS_AUTH_BEARER")]
    pub auth_bearer: Option<String>,
}

pub fn run(args: SocialArgs) -> Result<(), String> {
    match args.command {
        SocialCommand::Connect(connect_args) => run_connect(connect_args),
        SocialCommand::Publish(publish_args) => run_publish(publish_args),
    }
}

fn run_connect(args: ConnectArgs) -> Result<(), String> {
    let bearer = api_config::get_bearer_header(args.auth_bearer)
        .ok_or_else(|| "TELLERS_AUTH_BEARER required for social connect".to_string())?;
    let base = api_config::get_api_base();

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| format!("failed to build HTTP client: {}", e))?;

            let mut url = reqwest::Url::parse(&format!(
                "{}/social-publishing/connect",
                base.trim_end_matches('/')
            ))
            .map_err(|e| format!("invalid API base URL: {}", e))?;

            {
                let mut pairs = url.query_pairs_mut();
                for platform in &args.platforms {
                    pairs.append_pair("platforms", platform);
                }
                if let Some(redirect_url) = &args.redirect_url {
                    pairs.append_pair("redirect_url", redirect_url);
                }
                if args.show_calendar {
                    pairs.append_pair("show_calendar", "true");
                }
            }

            let resp = client
                .get(url)
                .header("authorization", &bearer)
                .send()
                .await
                .map_err(|e| format!("connect request failed: {}", e))?;

            if resp.status().is_redirection() {
                let location = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        format!(
                            "connect returned redirect without Location header (http_status: {})",
                            resp.status()
                        )
                    })?;
                println!("{}", location);
                return Ok(());
            }

            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if status.is_success() {
                if body.is_empty() {
                    return Err("connect succeeded but returned no redirect URL".to_string());
                }
                println!("{}", body);
                return Ok(());
            }

            Err(format!(
                "connect failed; http_status: {}; response: {}",
                status, body
            ))
        })
}

fn run_publish(args: PublishArgs) -> Result<(), String> {
    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(args.api_key)?;
    let bearer = api_config::get_bearer_header(args.auth_bearer);

    let mut body = SocialPublishPostRequest::new(args.asset_id, args.platforms);
    body.title = Some(args.title);
    body.description = Some(args.description);
    if let Some(scheduled_date) = args.scheduled_date {
        body.scheduled_date = Some(Some(scheduled_date));
    }

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            let resp = api::publish_social_post_social_publishing_posts_post(
                &cfg,
                body,
                Some(&api_key),
                bearer.as_deref(),
            )
            .await
            .map_err(|e| api_config::format_api_error(&e))?;

            println!("status: {}", resp.status);
            if let Some(request_id) = resp.request_id.and_then(|v| v) {
                println!("request_id: {}", request_id);
            }
            if let Some(job_id) = resp.job_id.and_then(|v| v) {
                println!("job_id: {}", job_id);
            }
            if let Some(provider_response) = resp.provider_response {
                println!(
                    "provider_response: {}",
                    serde_json::to_string_pretty(&provider_response)
                        .unwrap_or_else(|_| format!("{:?}", provider_response))
                );
            }
            Ok(())
        })
}
