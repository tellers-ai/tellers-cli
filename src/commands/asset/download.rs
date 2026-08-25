use std::path::PathBuf;

use clap::{Args, ValueEnum};
use tellers_api_client::apis::accepts_api_key_api as api;
use tellers_api_client::models::asset_download_url_batch_request::Quality;
use tellers_api_client::models::AssetDownloadUrlBatchRequest;
use tokio::io::AsyncWriteExt;

use crate::commands::api_config;

#[derive(Clone, Debug, ValueEnum)]
pub enum DownloadQuality {
    #[value(name = "480p")]
    P480,
    #[value(name = "720p")]
    P720,
    #[value(name = "1080p")]
    P1080,
    Original,
    Highest,
    Lowest,
}

impl From<DownloadQuality> for Quality {
    fn from(value: DownloadQuality) -> Self {
        match value {
            DownloadQuality::P480 => Self::Variant480p,
            DownloadQuality::P720 => Self::Variant720p,
            DownloadQuality::P1080 => Self::Variant1080p,
            DownloadQuality::Original => Self::Original,
            DownloadQuality::Highest => Self::Highest,
            DownloadQuality::Lowest => Self::Lowest,
        }
    }
}

#[derive(Args, Debug)]
pub struct DownloadArgs {
    /// Asset ID to download.
    pub asset_id: String,

    /// Local destination. Defaults to the asset ID in the current directory.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Video rendition to download.
    #[arg(long, value_enum, default_value = "highest")]
    pub quality: DownloadQuality,

    /// Replace an existing destination file.
    #[arg(long)]
    pub force: bool,

    #[arg(long, env = "TELLERS_API_KEY", hide = true)]
    pub api_key: Option<String>,

    #[arg(long, env = "TELLERS_AUTH_BEARER", hide = true)]
    pub auth_bearer: Option<String>,
}

pub fn run(args: DownloadArgs) -> Result<(), String> {
    let destination = args.output.unwrap_or_else(|| PathBuf::from(&args.asset_id));
    if destination.exists() && !args.force {
        return Err(format!(
            "destination already exists: {}; use --force to replace it",
            destination.display()
        ));
    }

    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(args.api_key)?;
    let bearer_header = api_config::get_bearer_header(args.auth_bearer);
    let asset_id = args.asset_id;
    let quality = args.quality.into();
    let partial = partial_path(&destination);

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            let mut request = AssetDownloadUrlBatchRequest::new(vec![asset_id.clone()]);
            request.quality = Some(quality);

            let response = api::get_asset_download_url_batch_json_asset_download_urls_post(
                &cfg,
                request,
                Some(&api_key),
                bearer_header.as_deref(),
            )
            .await
            .map_err(|e| format!("failed to resolve asset download URL: {}", e))?;

            let item = response
                .items
                .into_iter()
                .next()
                .ok_or_else(|| "download URL response contained no item".to_string())?;
            if item.error {
                return Err(format!(
                    "asset {} cannot be downloaded: {}",
                    asset_id,
                    item.error_message
                        .flatten()
                        .unwrap_or_else(|| "unknown error".to_string())
                ));
            }
            let url = item
                .download
                .flatten()
                .ok_or_else(|| format!("asset {} has no download URL", asset_id))?
                .url
                .to_string();

            let result = download_to_file(&url, &partial, &destination, args.force).await;
            if result.is_err() {
                let _ = tokio::fs::remove_file(&partial).await;
            }
            result
        })
}

fn partial_path(destination: &std::path::Path) -> PathBuf {
    let mut partial = destination.as_os_str().to_os_string();
    partial.push(".part");
    PathBuf::from(partial)
}

async fn download_to_file(
    url: &str,
    partial: &std::path::Path,
    destination: &std::path::Path,
    force: bool,
) -> Result<(), String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("download request failed: {}", e))?
        .error_for_status()
        .map_err(|e| format!("download request failed: {}", e))?;

    let mut file = tokio::fs::File::create(partial)
        .await
        .map_err(|e| format!("failed to create {}: {}", partial.display(), e))?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
        let chunk = chunk.map_err(|e| format!("download failed: {}", e))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("failed writing {}: {}", partial.display(), e))?;
    }
    file.flush()
        .await
        .map_err(|e| format!("failed flushing {}: {}", partial.display(), e))?;
    drop(file);

    if destination.exists() {
        if !force {
            return Err(format!(
                "destination appeared during download: {}; use --force to replace it",
                destination.display()
            ));
        }
        tokio::fs::remove_file(destination)
            .await
            .map_err(|e| format!("failed replacing {}: {}", destination.display(), e))?;
    }
    tokio::fs::rename(partial, destination)
        .await
        .map_err(|e| format!("failed finalizing {}: {}", destination.display(), e))?;
    println!("downloaded asset to {}", destination.display());
    Ok(())
}
