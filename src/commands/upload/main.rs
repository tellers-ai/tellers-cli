use clap::Args;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::auth;
use crate::media::ffmpeg::ensure_ffmpeg_available;
use crate::media::transcode::{
    convert_to_mp3, create_rendition, has_video_streams, is_mxf_file, Preset, RenditionDefinition,
};
use crate::media::video_file_ext::has_video_ext;
use crate::media::video_quality::parse_quality;
use crate::media::video_quality::VideoQuality;
use crate::output;
use crate::tui::ProgressHandle;
use crate::uploads_tracking;

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

    #[arg(long)]
    pub preset: Option<Preset>,

    pub path: String,

    #[arg(long)]
    pub in_app_path: Option<String>,

    #[arg(long, env = "TELLERS_AUTH_BEARER")]
    pub auth_bearer: Option<String>,

    #[arg(long, default_value_t = false)]
    pub force_upload: bool,

    #[arg(long, default_value_t = 4)]
    pub parallel_uploads: usize,

    #[arg(long, num_args = 1..)]
    pub ext: Vec<String>,

    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
}


struct FileToUpload {
    upload_path: PathBuf,
    original_path: PathBuf,
}

fn has_extension(file_path: &PathBuf, extensions: &[String]) -> bool {
    if extensions.is_empty() {
        return true;
    }

    let file_ext = file_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    extensions
        .iter()
        .any(|ext| ext.trim_start_matches('.').to_ascii_lowercase() == file_ext)
}

pub fn run(args: UploadArgs) -> Result<(), String> {
    let base_dir = PathBuf::from(&args.path);
    if !base_dir.exists() {
        return Err(format!("path not found: {}", base_dir.display()));
    }

    let mut original_files: Vec<PathBuf> = Vec::new();
    if base_dir.is_file() {
        original_files.push(base_dir.clone());
    } else {
        for entry in WalkDir::new(&base_dir).into_iter().filter_map(Result::ok) {
            let p = entry.path();
            if p.is_file() {
                original_files.push(p.to_path_buf());
            }
        }
    }

    if original_files.is_empty() {
        return Err("no files found to upload".to_string());
    }

    if !args.ext.is_empty() {
        let before_count = original_files.len();
        original_files.retain(|file_path| has_extension(file_path, &args.ext));
        let filtered_count = original_files.len();
        if filtered_count < before_count {
        output::info(format!(
            "Filtered to {} file(s) matching extensions: {}",
            filtered_count,
            args.ext
                .iter()
                .map(|e| e.trim_start_matches('.'))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        }
        if original_files.is_empty() {
            return Err(format!(
                "no files found with extensions: {}",
                args.ext
                    .iter()
                    .map(|e| e.trim_start_matches('.'))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    if args.dry_run {
        return super::dry_run::run_dry_run(
            &original_files,
            &base_dir,
            &args.in_app_path,
            &args.auth_bearer,
            args.force_upload,
        );
    }

    let mut files_to_upload: Vec<FileToUpload> = Vec::new();
    if args.local_encoding {
        ensure_ffmpeg_available()?;
        output::info(format!(
            "Local encoding enabled; generating renditions in temp dir: {}",
            args.qualities
                .iter()
                .map(|q| q.as_label())
                .collect::<Vec<_>>()
                .join(", ")
        ));

        for original_file in &original_files {
            if is_mxf_file(original_file) {
                match has_video_streams(original_file) {
                    Ok(true) => {
                        if args.qualities.len() > 1 {
                            return Err("Only supporting single quality for now".to_string());
                        }
                        output::info(format!(
                            "MXF file {} contains video, converting to MP4",
                            original_file.display()
                        ));
                        let encoded_file = create_rendition(
                            original_file,
                            RenditionDefinition {
                                quality: Some(args.qualities[0]),
                                preset: args.preset,
                                crf: None,
                                audio_bitrate: None,
                            },
                        )
                        .map_err(|e| {
                            format!(
                                "failed to encode MXF rendition for {}: {}",
                                original_file.display(),
                                e
                            )
                        })?;
                        files_to_upload.push(FileToUpload {
                            upload_path: encoded_file,
                            original_path: original_file.clone(),
                        });
                    }
                    Ok(false) => {
                        output::info(format!(
                            "MXF file {} is audio-only, converting to MP3",
                            original_file.display()
                        ));
                        let encoded_file = convert_to_mp3(original_file, None).map_err(|e| {
                            format!(
                                "failed to convert MXF to MP3 for {}: {}",
                                original_file.display(),
                                e
                            )
                        })?;
                        files_to_upload.push(FileToUpload {
                            upload_path: encoded_file,
                            original_path: original_file.clone(),
                        });
                    }
                    Err(e) => {
                        return Err(format!(
                            "failed to check video streams in MXF file {}: {}",
                            original_file.display(),
                            e
                        ));
                    }
                }
            } else if has_video_ext(original_file) {
                if args.qualities.len() > 1 {
                    return Err("Only supporting single quality for now".to_string());
                }
                let encoded_file = create_rendition(
                    original_file,
                    RenditionDefinition {
                        quality: Some(args.qualities[0]),
                        preset: args.preset,
                        crf: None,
                        audio_bitrate: None,
                    },
                )
                .map_err(|e| {
                    format!(
                        "failed to encode rendition for {}: {}",
                        original_file.display(),
                        e
                    )
                })?;
                files_to_upload.push(FileToUpload {
                    upload_path: encoded_file,
                    original_path: original_file.clone(),
                });
            } else {
                files_to_upload.push(FileToUpload {
                    upload_path: original_file.clone(),
                    original_path: original_file.clone(),
                });
            }
        }

        output::info(format!(
            "Prepared {} file(s) for upload (including renditions)",
            files_to_upload.len()
        ));
        for f in &files_to_upload {
            if let Ok(md) = std::fs::metadata(&f.upload_path) {
                output::item(format!("{} ({} bytes)", f.upload_path.display(), md.len()));
            } else {
                output::item(format!("{}", f.upload_path.display()));
            }
        }
    } else {
        output::info(format!("Discovered {} file(s) to upload", original_files.len()));
        for f in &original_files {
            if let Ok(md) = std::fs::metadata(f) {
                output::item(format!("{} ({} bytes)", f.display(), md.len()));
            } else {
                output::item(format!("{}", f.display()));
            }
        }
        for original_file in original_files {
            files_to_upload.push(FileToUpload {
                upload_path: original_file.clone(),
                original_path: original_file,
            });
        }
    }

    let api_base = std::env::var("TELLERS_API_BASE")
        .unwrap_or_else(|_| "https://api.prod.aws.tellers.ai".to_string());
    let api_key =
        std::env::var("TELLERS_API_KEY").map_err(|_| "TELLERS_API_KEY not set".to_string())?;

    let mut cfg = Configuration::default();
    cfg.base_path = api_base;
    output::info(format!("API base: {}", cfg.base_path));

    let bearer_env = args
        .auth_bearer
        .clone()
        .or_else(|| std::env::var("TELLERS_AUTH_BEARER").ok())
        .filter(|v| !v.is_empty());
    let bearer_header_for_auth = bearer_env.as_deref().map(|b| {
        if b.starts_with("Bearer ") {
            b.to_string()
        } else {
            format!("Bearer {}", b)
        }
    });
    let user_id = auth::get_user_id_from_bearer(bearer_header_for_auth.as_deref());

    if !args.force_upload {
        let original_count = files_to_upload.len();
        files_to_upload.retain(|file_info| {
            !super::utils::is_already_uploaded(
                &file_info.original_path,
                &user_id,
                &base_dir,
                &args.in_app_path,
            )
        });

        let skipped = original_count - files_to_upload.len();
        if skipped > 0 {
            output::info(format!("Skipped {} already uploaded file(s)", skipped));
        }

        if files_to_upload.is_empty() {
            return Err("no files to upload (all files were already uploaded)".to_string());
        }
    }

    let upload_request_id = Uuid::new_v4().to_string();

    let mut requests: Vec<AssetUploadRequest> = Vec::with_capacity(files_to_upload.len());
    let mut file_upload_ids: Vec<String> = Vec::with_capacity(files_to_upload.len());
    let mut file_in_app_paths: Vec<String> = Vec::with_capacity(files_to_upload.len());
    let mut upload_paths: Vec<PathBuf> = Vec::with_capacity(files_to_upload.len());
    for file_info in &files_to_upload {
        let content_length = std::fs::metadata(&file_info.upload_path)
            .map_err(|e| format!("failed to stat {}: {}", file_info.upload_path.display(), e))?
            .len();

        let upload_id = Uuid::new_v4().to_string();
        file_upload_ids.push(upload_id.clone());

        output::info(format!(
            "Build upload request: id={} file={} size={} bytes",
            upload_id,
            file_info.upload_path.display(),
            content_length
        ));

        let file_in_app_path =
            super::utils::compute_in_app_path(&file_info.original_path, &base_dir, &args.in_app_path);
        file_in_app_paths.push(file_in_app_path.clone());
        upload_paths.push(file_info.upload_path.clone());

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i32;

        let file_name_str = file_info
            .original_path
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
            let bearer_header = bearer_env.as_deref().map(|b| {
                if b.starts_with("Bearer ") {
                    b.to_string()
                } else {
                    format!("Bearer {}", b)
                }
            });
            let mut progress = crate::tui::InlineProgress::new("Uploading Files", upload_paths.len())?;
            let progress_handle = progress.clone_handle();
            let render_handle = progress.start_render_loop(progress_handle.clone());
            
            let _ = progress_handle.add_info(format!("Requesting presigned URLs for {} files...", requests.len()));
            
            let responses =
                request_presigned_urls(&cfg, &requests, &api_key, bearer_header.as_deref()).await?;
            let _ = progress_handle.add_info(format!("Received {} presigned URL(s)", responses.len()));

            let mut id_to_resp: HashMap<String, AssetUploadResponse> = HashMap::new();
            for r in responses.iter().cloned() {
                id_to_resp.insert(r.upload_id.clone(), r);
            }

            let upload_result = upload_to_presigned_urls(
                &upload_paths,
                &file_upload_ids,
                &file_in_app_paths,
                &id_to_resp,
                &upload_request_id,
                &user_id,
                args.parallel_uploads,
                &progress_handle,
            )
            .await;
            
            if let Err(ref e) = upload_result {
                let _ = progress_handle.add_error(format!("Upload failed: {}", e));
            }
            upload_result?;

            let _ = progress_handle.add_success("All uploads completed");
            
            // Call preprocess for uploaded assets
            let preproc_req = ProcessAssetsRequest::new(
                responses.clone(),
                None::<tellers_api_client::models::VersionReference>,
            );
            let _ = progress_handle.add_info(format!(
                "Triggering preprocessing for {} asset(s)...",
                preproc_req.assets.len()
            ));
            let preproc_tasks = api::process_assets_users_assets_preprocess_post(
                &cfg,
                preproc_req,
                None,
                Some(&api_key),
                bearer_header.as_deref(),
            )
            .await
            .map_err(|e| format!("failed to trigger preprocess: {}", e))?;
            let _ = progress_handle.add_success(format!("Preprocess tasks queued: {}", preproc_tasks.len()));
            
            // Small delay to ensure last message is rendered
            tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
            
            render_handle.abort();
            let _ = render_handle.await;
            progress.finish()?;
            
            // Add empty line after progress display
            println!();

            Ok(())
        })
}

async fn request_presigned_urls(
    cfg: &Configuration,
    requests: &Vec<AssetUploadRequest>,
    api_key: &str,
    bearer_opt: Option<&str>,
) -> Result<Vec<AssetUploadResponse>, String> {
    if std::env::var("TELLERS_DEBUG_HTTP").ok().as_deref() == Some("1") {
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
    file_in_app_paths: &Vec<String>,
    id_to_resp: &HashMap<String, AssetUploadResponse>,
    upload_request_id: &str,
    user_id: &str,
    max_concurrent: usize,
    progress_handle: &ProgressHandle,
) -> Result<(), String> {
    let http = Arc::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| format!("failed to build http client: {}", e))?,
    );

    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let mut upload_tasks = Vec::new();

    for (i, file_path) in files.iter().enumerate() {
        let file_path = file_path.clone();
        let upload_id = file_upload_ids[i].clone();
        let in_app_path = file_in_app_paths[i].clone();
        let upload_resp = id_to_resp
            .get(&upload_id)
            .ok_or_else(|| format!("missing presigned url for upload_id {}", upload_id))?
            .clone();
        let http_clone = Arc::clone(&http);
        let semaphore_clone = Arc::clone(&semaphore);
        let progress_handle_clone = progress_handle.clone();
        let user_id = user_id.to_string();
        let upload_request_id = upload_request_id.to_string();
        let task_id = i;

        let file_name = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let file_size = std::fs::metadata(&file_path)
            .map_err(|e| format!("failed to stat {}: {}", file_path.display(), e))?
            .len();

        let _ = progress_handle_clone.start_task(task_id, file_name.clone(), file_size);

        let task = tokio::spawn(async move {
            let _permit = semaphore_clone
                .acquire()
                .await
                .map_err(|e| format!("failed to acquire semaphore: {}", e))?;

            let result = upload_single_file(
                &file_path,
                &upload_id,
                &upload_resp,
                &in_app_path,
                &upload_request_id,
                &user_id,
                task_id,
                &http_clone,
                &progress_handle_clone,
                file_size,
            )
            .await;

            let success = result.is_ok();
            let _ = progress_handle_clone.finish_task(task_id, success);
            result
        });

        upload_tasks.push(task);
    }

    for task in upload_tasks {
        task.await
            .map_err(|e| format!("upload task panicked: {}", e))?
            .map_err(|e| format!("upload failed: {}", e))?;
    }

    Ok(())
}

async fn upload_single_file(
    file_path: &PathBuf,
    _upload_id: &str,
    upload_resp: &AssetUploadResponse,
    in_app_path: &str,
    upload_request_id: &str,
    user_id: &str,
    task_id: usize,
    http: &reqwest::Client,
    progress_handle: &ProgressHandle,
    total_bytes: u64,
) -> Result<(), String> {
    let upload_url = upload_resp.presigned_put_url.clone();

    let mut f = File::open(file_path)
        .map_err(|e| format!("failed to open {}: {}", file_path.display(), e))?;
    let mut buf = Vec::with_capacity(total_bytes as usize);
    
    const CHUNK_SIZE: usize = 1024 * 1024; // 1MB chunks
    let mut uploaded = 0u64;
    let mut chunk = vec![0u8; CHUNK_SIZE.min(total_bytes as usize)];
    
    loop {
        let n = f.read(&mut chunk)
            .map_err(|e| format!("failed to read {}: {}", file_path.display(), e))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        uploaded += n as u64;
        let _ = progress_handle.update_task(task_id, uploaded);
    }

    let content_type = mime_guess::from_path(file_path)
        .first_or_text_plain()
        .essence_str()
        .to_string();

    let put_res = http
        .put(upload_url)
        .header(reqwest::header::CONTENT_LENGTH, total_bytes)
        .header(reqwest::header::CONTENT_TYPE, &content_type)
        .body(buf)
        .send()
        .await
        .map_err(|e| format!("upload failed for {}: {}", file_path.display(), e))?;

    let _ = progress_handle.update_task(task_id, total_bytes);

    if !put_res.status().is_success() {
        let status = put_res.status();
        let body = put_res
            .text()
            .await
            .unwrap_or_else(|_| "<failed to read error body>".to_string());
        let error_msg = format!(
            "Upload failed for {}: HTTP {} - {}",
            file_path.display(),
            status,
            body
        );
        let _ = progress_handle.add_error(error_msg.clone());
        return Err(error_msg);
    }

    if let Err(e) = uploads_tracking::record_upload(
        user_id,
        file_path,
        in_app_path,
        &upload_resp.asset_id,
        upload_request_id,
    ) {
        let _ = progress_handle.add_warning(format!("Failed to record upload in tracking file: {}", e));
    }

    Ok(())
}
