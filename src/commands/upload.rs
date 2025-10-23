use clap::Args;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use uuid::Uuid;
use walkdir::WalkDir;

use tellers_api_client::apis::auth_required_api as api;
use tellers_api_client::apis::configuration::Configuration;
use tellers_api_client::models::{AssetUploadRequest, AssetUploadResponse, ProcessAssetsRequest, SourceFileInfo};

#[derive(Args, Debug)]
pub struct UploadArgs {
    /// Only upload proxies/metadata for the media (no full content)
    #[arg(long, default_value_t = false)]
    pub only_proxies: bool,

    /// Path to media folder to upload
    pub path: String,
}

pub fn run(args: UploadArgs) -> Result<(), String> {
    let base_dir = PathBuf::from(&args.path);
    if !base_dir.exists() {
        return Err(format!("path not found: {}", base_dir.display()));
    }

    let mut files: Vec<PathBuf> = Vec::new();
    if base_dir.is_file() {
        files.push(base_dir.clone());
    } else {
        for entry in WalkDir::new(&base_dir).into_iter().filter_map(Result::ok) {
            let p = entry.path();
            if p.is_file() {
                files.push(p.to_path_buf());
            }
        }
    }

    if files.is_empty() {
        return Err("no files found to upload".to_string());
    }

    let api_base =
        std::env::var("TELLERS_API_BASE").unwrap_or_else(|_| "https://api.tellers.ai".to_string());
    let api_key =
        std::env::var("TELLERS_API_KEY").map_err(|_| "TELLERS_API_KEY not set".to_string())?;

    let mut cfg = Configuration::default();
    cfg.base_path = api_base;

    // Build upload requests
    let mut requests: Vec<AssetUploadRequest> = Vec::with_capacity(files.len());
    let mut file_upload_ids: Vec<String> = Vec::with_capacity(files.len());
    for file_path in &files {
        let content_length = std::fs::metadata(file_path)
            .map_err(|e| format!("failed to stat {}: {}", file_path.display(), e))?
            .len();

        let upload_id = Uuid::new_v4().to_string();
        file_upload_ids.push(upload_id.clone());

        let rel_path = file_path
            .strip_prefix(&base_dir)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string();

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i32;

        let source_info = SourceFileInfo::new(
            file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            None,
            None,
            vec![],
            None,
            now_secs,
            vec![rel_path.clone()],
            Some(rel_path.clone()),
            None,
            vec![],
        );

        let req = AssetUploadRequest::new(
            i32::try_from(content_length).unwrap_or(i32::MAX),
            upload_id,
            source_info,
        );
        requests.push(req);
    }

    // Perform network calls in async runtime
    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            // 1) Get presigned URLs
            let responses = api::create_upload_urls_users_assets_upload_urls_post(
                &cfg,
                requests,
                Some(&api_key),
                None,
            )
            .await
            .map_err(|e| format!("failed to get upload urls: {}", e))?;

            // Map upload_id -> response
            let mut id_to_resp: std::collections::HashMap<String, AssetUploadResponse> = std::collections::HashMap::new();
            for r in responses.iter().cloned() {
                id_to_resp.insert(r.upload_id.clone(), r);
            }

            // 2) Upload each file via PUT to presigned URL
            let http = reqwest::Client::new();
            for (i, file_path) in files.iter().enumerate() {
                let content_length = std::fs::metadata(file_path)
                    .map_err(|e| format!("failed to stat {}: {}", file_path.display(), e))?
                    .len();

                let upload_id = &file_upload_ids[i];
                let upload_resp = id_to_resp.get(upload_id)
                    .ok_or_else(|| format!("missing presigned url for upload_id {}", upload_id))?;
                let upload_url = upload_resp.presigned_put_url.clone();

                let mut f = File::open(file_path)
                    .map_err(|e| format!("failed to open {}: {}", file_path.display(), e))?;
                let mut buf = Vec::with_capacity(content_length as usize);
                f.read_to_end(&mut buf)
                    .map_err(|e| format!("failed to read {}: {}", file_path.display(), e))?;

                let content_type = mime_guess::from_path(file_path)
                    .first_or_text_plain()
                    .essence_str()
                    .to_string();

                let put_res = http
                    .put(upload_url)
                    .header(reqwest::header::CONTENT_LENGTH, content_length)
                    .header(reqwest::header::CONTENT_TYPE, content_type)
                    .body(buf)
                    .send()
                    .await
                    .map_err(|e| format!("upload failed for {}: {}", file_path.display(), e))?;

                if !put_res.status().is_success() {
                    return Err(format!(
                        "upload failed: {} -> status {}",
                        file_path.display(),
                        put_res.status()
                    ));
                }
                println!("uploaded: {}", file_path.display());
            }

            // Optionally call preprocess endpoint when not only_proxies
            if !args.only_proxies {
                let mut assets_for_processing: Vec<AssetUploadResponse> = Vec::new();
                for uid in &file_upload_ids {
                    if let Some(r) = id_to_resp.get(uid) {
                        assets_for_processing.push(r.clone());
                    }
                }
                if !assets_for_processing.is_empty() {
                    let preprocess_req = ProcessAssetsRequest::new(assets_for_processing, None);
                    let _ = api::process_assets_users_assets_preprocess_post(&cfg, preprocess_req, None, Some(&api_key), None).await
                        .map_err(|e| format!("preprocess request failed: {}", e))?;
                }
            }
            Ok(())
        })
}
