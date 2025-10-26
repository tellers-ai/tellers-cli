use clap::Args;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use uuid::Uuid;
use walkdir::WalkDir;

use tellers_api_client::apis::auth_required_api as api;
use tellers_api_client::apis::configuration::Configuration;
use tellers_api_client::models::{
    AssetUploadRequest, AssetUploadResponse, ProcessAssetsRequest, SourceFileInfo,
};

#[derive(Args, Debug)]
pub struct UploadArgs {
    #[arg(long, default_value_t = false)]
    pub only_proxies: bool,

    pub path: String,

    #[arg(long)]
    pub in_app_path: Option<String>,

    #[arg(long, env = "TELLERS_AUTH_BEARER")]
    pub auth_bearer: Option<String>,
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

    println!("discovered {} file(s) to upload", files.len());
    for f in &files {
        if let Ok(md) = std::fs::metadata(f) {
            println!("  - {} ({} bytes)", f.display(), md.len());
        } else {
            println!("  - {}", f.display());
        }
    }

    let api_base = std::env::var("TELLERS_API_BASE")
        .unwrap_or_else(|_| "https://api.prod.aws.tellers.ai".to_string());
    let api_key =
        std::env::var("TELLERS_API_KEY").map_err(|_| "TELLERS_API_KEY not set".to_string())?;

    let mut cfg = Configuration::default();
    cfg.base_path = api_base;
    println!("api base: {}", cfg.base_path);

    let mut requests: Vec<AssetUploadRequest> = Vec::with_capacity(files.len());
    let mut file_upload_ids: Vec<String> = Vec::with_capacity(files.len());
    for file_path in &files {
        let content_length = std::fs::metadata(file_path)
            .map_err(|e| format!("failed to stat {}: {}", file_path.display(), e))?
            .len();

        let upload_id = Uuid::new_v4().to_string();
        file_upload_ids.push(upload_id.clone());

        println!(
            "build upload request: id={} file={} size={} bytes",
            upload_id,
            file_path.display(),
            content_length
        );

        let rel_path = if base_dir.is_dir() {
            file_path
                .strip_prefix(&base_dir)
                .unwrap_or(file_path)
                .to_string_lossy()
                .to_string()
        } else {
            file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        };

        let file_in_app_path = match &args.in_app_path {
            Some(prefix) if !prefix.is_empty() => {
                if base_dir.is_dir() {
                    format!("{}/{}", prefix.trim_end_matches('/'), rel_path)
                } else {
                    prefix.clone()
                }
            }
            _ => rel_path.clone(),
        };

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i32;

        let file_name_str = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let source_info = SourceFileInfo::new(
            "__user_upload__".to_string(),
            None,
            None,
            vec!["__current_user__".to_string()],
            Some(now_secs),
            now_secs,
            vec![file_in_app_path.clone()],
            Some(file_name_str),
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

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            // 1) Get presigned URLs
            println!(
                "requesting presigned URLs for {} asset(s)...",
                requests.len()
            );
            let bearer = args
                .auth_bearer
                .clone()
                .or_else(|| std::env::var("TELLERS_AUTH_BEARER").ok())
                .filter(|v| !v.is_empty());
            if bearer.is_some() {
                println!("using Authorization: Bearer ... from TELLERS_AUTH_BEARER");
            }
            println!(
                "HTTP POST {}",
                format!("{}/users/assets/upload_urls", cfg.base_path)
            );
            println!(
                "headers: x-api-key={}, authorization={}",
                "set",
                if bearer.is_some() { "set" } else { "unset" }
            );
            println!("query params: (none)");
            for (idx, r) in requests.iter().enumerate() {
                println!(
                    "  asset[{}]: upload_id={} content_length={} in_app_path={}",
                    idx,
                    r.upload_id,
                    r.content_length,
                    r.source_file
                        .in_app_path
                        .get(0)
                        .cloned()
                        .unwrap_or_default()
                );
            }
            let bearer_header = bearer.as_deref().map(|b| {
                if b.starts_with("Bearer ") {
                    b.to_string()
                } else {
                    format!("Bearer {}", b)
                }
            });
            let bearer_opt = bearer_header.as_deref();
            let responses = api::create_upload_urls_users_assets_upload_urls_post(
                &cfg,
                requests,
                Some(&api_key),
                bearer_opt,
            )
            .await
            .map_err(|e| {
                let mut m = format!("failed to get upload urls: {}", e);
                match &e {
                    tellers_api_client::apis::Error::Reqwest(req_err) => {
                        if let Some(status) = req_err.status() {
                            m.push_str(&format!("; http_status: {}", status));
                        }
                        if req_err.is_builder() {
                            m.push_str("; reqwest builder error");
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
                let mut src_opt = std::error::Error::source(&e);
                while let Some(src) = src_opt {
                    m.push_str(&format!("; source: {}", src));
                    src_opt = src.source();
                }
                m
            })?;
            println!("received {} presigned URL(s)", responses.len());

            let mut id_to_resp: std::collections::HashMap<String, AssetUploadResponse> =
                std::collections::HashMap::new();
            for r in responses.iter().cloned() {
                id_to_resp.insert(r.upload_id.clone(), r);
            }

            println!("resolved upload ids and asset ids:");
            for (idx, uid) in file_upload_ids.iter().enumerate() {
                if let Some(r) = id_to_resp.get(uid) {
                    println!(
                        "  [{}] upload_id={} asset_id={}",
                        idx, r.upload_id, r.asset_id
                    );
                } else {
                    println!("  [{}] upload_id={} asset_id=<missing>", idx, uid);
                }
            }

            let http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .map_err(|e| format!("failed to build http client: {}", e))?;
            for (i, file_path) in files.iter().enumerate() {
                let content_length = std::fs::metadata(file_path)
                    .map_err(|e| format!("failed to stat {}: {}", file_path.display(), e))?
                    .len();

                let upload_id = &file_upload_ids[i];
                let upload_resp = id_to_resp
                    .get(upload_id)
                    .ok_or_else(|| format!("missing presigned url for upload_id {}", upload_id))?;
                let upload_url = upload_resp.presigned_put_url.clone();
                let host = url::Url::parse(&upload_url)
                    .ok()
                    .and_then(|u| u.host_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| "<unknown-host>".to_string());

                println!(
                    "uploading [{} / {}]: id={} asset={} file={} size={} bytes -> {}",
                    i + 1,
                    files.len(),
                    upload_id,
                    upload_resp.asset_id,
                    file_path.display(),
                    content_length,
                    host
                );

                let mut f = File::open(file_path)
                    .map_err(|e| format!("failed to open {}: {}", file_path.display(), e))?;
                let mut buf = Vec::with_capacity(content_length as usize);
                f.read_to_end(&mut buf)
                    .map_err(|e| format!("failed to read {}: {}", file_path.display(), e))?;

                let content_type = mime_guess::from_path(file_path)
                    .first_or_text_plain()
                    .essence_str()
                    .to_string();
                println!("  content-type: {}", content_type);

                let started_at = std::time::Instant::now();
                let put_res = http
                    .put(upload_url)
                    .header(reqwest::header::CONTENT_LENGTH, content_length)
                    .header(reqwest::header::CONTENT_TYPE, content_type)
                    .body(buf)
                    .send()
                    .await
                    .map_err(|e| format!("upload failed for {}: {}", file_path.display(), e))?;

                if !put_res.status().is_success() {
                    let status = put_res.status();
                    let body = put_res
                        .text()
                        .await
                        .unwrap_or_else(|_| "<failed to read error body>".to_string());
                    return Err(format!(
                        "upload failed: {} -> status {} body: {}",
                        file_path.display(),
                        status,
                        body
                    ));
                }
                let elapsed = started_at.elapsed();
                println!(
                    "  uploaded successfully: {} ({}.{:03}s)",
                    file_path.display(),
                    elapsed.as_secs(),
                    elapsed.subsec_millis()
                );
            }
            Ok(())
        })
}
