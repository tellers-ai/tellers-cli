use clap::Args;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::video::ffmpeg::ensure_ffmpeg_available;
use crate::video::transcode::{create_rendition, RenditionDefinition};
use crate::video::video_file_ext::has_video_ext;
use crate::video::video_quality::parse_quality;
use crate::video::video_quality::VideoQuality;
use tellers_api_client::apis::auth_required_api as api;
use tellers_api_client::apis::configuration::Configuration;
use tellers_api_client::models::{
    AssetUploadRequest, AssetUploadResponse, ProcessAssetsRequest, SourceFileInfo,
};

#[derive(Args, Debug)]
pub struct UploadArgs {
    #[arg(long, default_value_t = false)]
    pub local_encoding: bool,

    #[arg(long, num_args = 1.., value_parser = parse_quality, default_values_t = vec![VideoQuality::P1080])]
    pub qualities: Vec<VideoQuality>,

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

    if args.local_encoding {
        ensure_ffmpeg_available()?;
        println!(
            "local encoding enabled; generating renditions in temp dir: {}",
            args.qualities
                .iter()
                .map(|q| q.as_label())
                .collect::<Vec<_>>()
                .join(", ")
        );

        let mut encoded: Vec<PathBuf> = Vec::new();
        for f in &files {
            if has_video_ext(f) {
                if args.qualities.len() > 1 {
                    return Err("Only supporting single quality for now".to_string());
                }
                let out = create_rendition(
                    f,
                    RenditionDefinition {
                        quality: Some(args.qualities[0]),
                        preset: None,
                        crf: None,
                        audio_bitrate: None,
                    },
                )
                .map_err(|e| format!("failed to encode rendition for {}: {}", f.display(), e))?;
                encoded.push(out);
            } else {
                encoded.push(f.clone());
            }
        }

        println!(
            "prepared {} file(s) for upload (including renditions)",
            encoded.len()
        );
        for f in &encoded {
            if let Ok(md) = std::fs::metadata(f) {
                println!("  - {} ({} bytes)", f.display(), md.len());
            } else {
                println!("  - {}", f.display());
            }
        }
        files = encoded;
    } else {
        println!("discovered {} file(s) to upload", files.len());
        for f in &files {
            if let Ok(md) = std::fs::metadata(f) {
                println!("  - {} ({} bytes)", f.display(), md.len());
            } else {
                println!("  - {}", f.display());
            }
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
            let bearer_env = args
                .auth_bearer
                .clone()
                .or_else(|| std::env::var("TELLERS_AUTH_BEARER").ok())
                .filter(|v| !v.is_empty());
            let bearer_header = bearer_env.as_deref().map(|b| {
                if b.starts_with("Bearer ") {
                    b.to_string()
                } else {
                    format!("Bearer {}", b)
                }
            });
            let responses =
                request_presigned_urls(&cfg, &requests, &api_key, bearer_header.as_deref()).await?;
            println!("received {} presigned URL(s)", responses.len());

            let mut id_to_resp: HashMap<String, AssetUploadResponse> = HashMap::new();
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

            upload_to_presigned_urls(&files, &file_upload_ids, &id_to_resp).await?;

            // Call preprocess for uploaded assets
            let preproc_req = ProcessAssetsRequest::new(
                responses.clone(),
                None::<tellers_api_client::models::VersionReference>,
            );
            println!(
                "triggering preprocessing for {} asset(s)...",
                preproc_req.assets.len()
            );
            let preproc_tasks = api::process_assets_users_assets_preprocess_post(
                &cfg,
                preproc_req,
                None,
                Some(&api_key),
                bearer_header.as_deref(),
            )
            .await
            .map_err(|e| format!("failed to trigger preprocess: {}", e))?;
            println!("preprocess tasks queued: {}", preproc_tasks.len());

            Ok(())
        })
}

async fn request_presigned_urls(
    cfg: &Configuration,
    requests: &Vec<AssetUploadRequest>,
    api_key: &str,
    bearer_opt: Option<&str>,
) -> Result<Vec<AssetUploadResponse>, String> {
    println!(
        "requesting presigned URLs for {} asset(s)...",
        requests.len()
    );
    println!(
        "HTTP POST {}",
        format!("{}/users/assets/upload_urls", cfg.base_path)
    );
    println!(
        "headers: x-api-key={}, authorization={}",
        "set",
        if bearer_opt.is_some() { "set" } else { "unset" }
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

    api::create_upload_urls_users_assets_upload_urls_post(
        cfg,
        requests.clone(),
        Some(api_key),
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
    })
}

async fn upload_to_presigned_urls(
    files: &Vec<PathBuf>,
    file_upload_ids: &Vec<String>,
    id_to_resp: &HashMap<String, AssetUploadResponse>,
) -> Result<(), String> {
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
}
