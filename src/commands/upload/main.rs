use clap::Args;
use regex::Regex;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::auth;
use crate::commands::api_config;
use crate::media::ffmpeg::ensure_ffmpeg_available;
use crate::media::metadata::extract_media_metadata;
use crate::media::media_file_type::is_audio_file;
use crate::media::transcode::{
    convert_to_mp3, create_rendition, has_video_streams, is_mxf_file, normalize_audio_to_mp3,
    Preset, RenditionDefinition,
};
use crate::media::video_file_ext::has_video_ext;
use crate::media::video_quality::parse_quality;
use crate::media::video_quality::VideoQuality;
use crate::output;
use crate::tui::{ProgressHandle, TwoQueueProgress, TwoQueueProgressHandle};
use crate::uploads_tracking;
use tokio::sync::mpsc as tokio_mpsc;

use tellers_api_client::apis::accepts_api_key_api as api;
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

    #[arg(long, num_args = 1..)]
    pub regex: Vec<String>,

    #[arg(long, default_value_t = false)]
    pub dry_run: bool,

    #[arg(long, default_value_t = false)]
    pub disable_description_generation: bool,
}

struct FileToUpload {
    upload_path: PathBuf,
    original_path: PathBuf,
}

enum DownscaleWork {
    MxfVideo(PathBuf),
    MxfAudio(PathBuf),
    Video(PathBuf),
    Audio(PathBuf),
    Passthrough(PathBuf),
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

fn matches_regex(file_path: &PathBuf, regex_patterns: &[Regex]) -> bool {
    if regex_patterns.is_empty() {
        return true;
    }

    let path_str = file_path.to_string_lossy();
    regex_patterns.iter().any(|re| re.is_match(&path_str))
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

    if !args.regex.is_empty() {
        let regex_patterns: Result<Vec<Regex>, _> = args
            .regex
            .iter()
            .map(|pattern| Regex::new(pattern))
            .collect();
        let regex_patterns = regex_patterns
            .map_err(|e| format!("invalid regex pattern: {}", e))?;

        let before_count = original_files.len();
        original_files.retain(|file_path| matches_regex(file_path, &regex_patterns));
        let filtered_count = original_files.len();
        if filtered_count < before_count {
            output::info(format!(
                "Filtered to {} file(s) matching regex patterns: {}",
                filtered_count,
                args.regex.join(", ")
            ));
        }
        if original_files.is_empty() {
            return Err(format!(
                "no files found matching regex patterns: {}",
                args.regex.join(", ")
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

    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(None)?;
    output::info(format!("API base: {}", cfg.base_path));

    let bearer_header_for_auth = api_config::get_bearer_header(args.auth_bearer.clone());
    let user_id = auth::get_user_id_from_bearer(bearer_header_for_auth.as_deref());

    if !args.force_upload {
        let before = original_files.len();
        original_files.retain(|path| {
            !super::utils::is_already_uploaded(path, &user_id, &base_dir, &args.in_app_path)
        });
        let skipped = before - original_files.len();
        if skipped > 0 {
            output::info(format!("Skipped {} already uploaded file(s)", skipped));
        }
        if original_files.is_empty() {
            return Err("no files to upload (all files were already uploaded)".to_string());
        }
    }

    let upload_request_id = Uuid::new_v4().to_string();

    if args.local_encoding {
        ensure_ffmpeg_available()?;
        output::info(format!(
            "Local encoding: downscale + upload queues ({} quality)",
            args.qualities
                .iter()
                .map(|q| q.as_label())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if args.qualities.len() > 1 {
            return Err("Only supporting single quality for now".to_string());
        }
        let work_items: Vec<DownscaleWork> = build_downscale_work(&original_files)?;
        output::info(format!("{} file(s) in downscale queue", work_items.len()));
        return run_two_queue_pipeline(
            work_items,
            &base_dir,
            &args,
            &cfg,
            &api_key,
            bearer_header_for_auth.as_deref(),
            &user_id,
            &upload_request_id,
        );
    }

    let mut files_to_upload: Vec<FileToUpload> = Vec::new();
    output::info(format!(
        "Discovered {} file(s) to upload",
        original_files.len()
    ));
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

        let file_in_app_path = super::utils::compute_in_app_path(
            &file_info.original_path,
            &base_dir,
            &args.in_app_path,
        );
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
        let umid = extract_media_metadata(&file_info.original_path)
            .ok();
        let mut source_info = SourceFileInfo::new(
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

        if let Some(metadata) = umid {
            if let Some(umid_value) = metadata.material_package_umid {
                source_info.capture_device_umid = Some(Some(umid_value));
            }
            if let Some(first_umid) = metadata.file_package_umids.first() {
                source_info.umid = Some(Some(first_umid.clone()));
            }
        }

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
            let bearer_header = api_config::get_bearer_header(args.auth_bearer.clone());
            let mut progress =
                crate::tui::InlineProgress::new("Uploading Files", upload_paths.len())?;
            let progress_handle = progress.clone_handle();
            let render_handle = progress.start_render_loop(progress_handle.clone());

            let _ = progress_handle.add_info(format!(
                "Requesting presigned URLs for {} files...",
                requests.len()
            ));

            let responses =
                request_presigned_urls(&cfg, &requests, &api_key, bearer_header.as_deref()).await?;
            let _ =
                progress_handle.add_info(format!("Received {} presigned URL(s)", responses.len()));

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
            let mut preproc_req = ProcessAssetsRequest::new(
                responses.clone(),
                None::<tellers_api_client::models::VersionReference>,
            );
            preproc_req.generate_time_based_media_description =
                Some(!args.disable_description_generation);
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
            let _ = progress_handle
                .add_success(format!("Preprocess tasks queued: {}", preproc_tasks.len()));

            crate::tui::InlineProgress::stop_render_loop(render_handle).await;
            progress.finish()?;

            // Add empty line after progress display
            println!();

            Ok(())
        })
}

fn work_item_file_name(w: &DownscaleWork) -> String {
    let p = match w {
        DownscaleWork::MxfVideo(p) | DownscaleWork::MxfAudio(p) | DownscaleWork::Video(p) | DownscaleWork::Audio(p) | DownscaleWork::Passthrough(p) => p,
    };
    p.file_name().unwrap_or_default().to_string_lossy().to_string()
}

fn do_one_downscale(
    work: DownscaleWork,
    progress_handle: &TwoQueueProgressHandle,
    qualities: &[VideoQuality],
    preset: Option<Preset>,
) -> Result<Option<FileToUpload>, String> {
    let file_to_upload = match work {
        DownscaleWork::MxfVideo(original_path) => {
            let def = RenditionDefinition {
                quality: Some(qualities[0]),
                preset,
                crf: None,
                audio_bitrate: None,
            };
            match create_rendition(&original_path, def) {
                Ok(upload_path) => FileToUpload { upload_path, original_path },
                Err(e) => {
                    let _ = progress_handle.add_error(format!(
                        "Downscale failed for {}: {}",
                        original_path.display(),
                        e
                    ));
                    return Ok(None);
                }
            }
        }
        DownscaleWork::MxfAudio(original_path) => match convert_to_mp3(&original_path, None) {
            Ok(upload_path) => FileToUpload { upload_path, original_path },
            Err(e) => {
                let _ = progress_handle.add_error(format!(
                    "MXF to MP3 failed for {}: {}",
                    original_path.display(),
                    e
                ));
                return Ok(None);
            }
        },
                DownscaleWork::Video(original_path) => {
                    let def = RenditionDefinition {
                        quality: Some(qualities[0]),
                        preset,
                        crf: None,
                        audio_bitrate: None,
                    };
                    match create_rendition(&original_path, def) {
                        Ok(upload_path) => FileToUpload { upload_path, original_path },
                        Err(e) => {
                            let _ = progress_handle.add_error(format!(
                                "Downscale failed for {}: {}",
                                original_path.display(),
                                e
                            ));
                            return Ok(None);
                        }
                    }
                }
                DownscaleWork::Audio(original_path) => match normalize_audio_to_mp3(&original_path, Some(192)) {
                    Ok(upload_path) => FileToUpload {
                        upload_path,
                        original_path,
                    },
                    Err(e) => {
                        let _ = progress_handle.add_error(format!(
                            "Audio normalization failed for {}: {}",
                            original_path.display(),
                            e
                        ));
                        return Ok(None);
                    }
                },
                DownscaleWork::Passthrough(original_path) => FileToUpload {
            upload_path: original_path.clone(),
            original_path,
        },
    };
    Ok(Some(file_to_upload))
}

fn build_downscale_work(original_files: &[PathBuf]) -> Result<Vec<DownscaleWork>, String> {
    let mut work = Vec::with_capacity(original_files.len());
    for path in original_files {
        if is_mxf_file(path) {
            match has_video_streams(path) {
                Ok(true) => work.push(DownscaleWork::MxfVideo(path.to_path_buf())),
                Ok(false) => work.push(DownscaleWork::MxfAudio(path.to_path_buf())),
                Err(e) => {
                    return Err(format!(
                        "failed to check video streams in MXF {}: {}",
                        path.display(),
                        e
                    ));
                }
            }
        } else if has_video_ext(path) {
            work.push(DownscaleWork::Video(path.to_path_buf()));
        } else if is_audio_file(path) {
            work.push(DownscaleWork::Audio(path.to_path_buf()));
        } else {
            work.push(DownscaleWork::Passthrough(path.to_path_buf()));
        }
    }
    Ok(work)
}

fn run_two_queue_pipeline(
    work_items: Vec<DownscaleWork>,
    base_dir: &PathBuf,
    args: &UploadArgs,
    cfg: &Configuration,
    api_key: &str,
    bearer_opt: Option<&str>,
    user_id: &str,
    upload_request_id: &str,
) -> Result<(), String> {
    let (upload_tx, mut upload_rx) = tokio_mpsc::channel::<FileToUpload>(64);

    let mut progress = TwoQueueProgress::new()?;
    let progress_handle = progress.clone_handle();
    progress_handle.set_downscale_queued(work_items.len());
    let downscale_pending_names: Vec<String> =
        work_items.iter().map(work_item_file_name).collect();
    progress_handle.set_downscale_pending(downscale_pending_names);

    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?;

    let base_dir = base_dir.clone();
    let qualities = args.qualities.clone();
    let preset = args.preset;

    let cfg = cfg.clone();
    let api_key = api_key.to_string();
    let bearer = bearer_opt.map(String::from);
    let base_dir_async = base_dir.clone();
    let in_app_path = args.in_app_path.clone();
    let user_id = user_id.to_string();
    let upload_request_id = upload_request_id.to_string();
    let disable_description_generation = args.disable_description_generation;

    let block_result = rt.block_on(async move {
        // Start render loop inside runtime so tokio::spawn has a current runtime
        let render_handle = progress.start_render_loop(progress_handle.clone());

        let http = Arc::new(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .map_err(|e| format!("failed to build http client: {}", e))?,
        );
        let bearer_header = bearer.as_deref();

        let progress_producer = progress_handle.clone();
        let upload_tx_producer = upload_tx.clone();
        let producer = async move {
            for w in work_items {
                progress_producer.decrement_downscale_queued();
                progress_producer.pop_downscale_pending();
                let name = work_item_file_name(&w);
                progress_producer.set_downscale_current(Some(name));
                let ph = progress_producer.clone();
                let qual = qualities.clone();
                let file = tokio::task::spawn_blocking(move || do_one_downscale(w, &ph, &qual, preset))
                    .await
                    .map_err(|e| format!("downscale task join: {}", e))??;
                progress_producer.set_downscale_current(None::<&str>);
                if let Some(f) = file {
                    progress_producer.increment_upload_queued();
                    let upload_name = f
                        .original_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    progress_producer.push_upload_pending(upload_name);
                    upload_tx_producer.send(f).await.map_err(|_| "upload channel closed".to_string())?;
                }
            }
            drop(upload_tx_producer);
            Ok::<(), String>(())
        };
        drop(upload_tx);

        let consumer = async move {
            let mut completed_responses: Vec<AssetUploadResponse> = Vec::new();
            while let Some(file_info) = upload_rx.recv().await {
                progress_handle.decrement_upload_queued();
                progress_handle.pop_upload_pending();
                let file_name = file_info
                    .original_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                progress_handle.set_upload_current(Some(file_name.clone()));

                let (req, _upload_id, in_app_path_str) = build_single_upload_request(
                    &file_info,
                    &base_dir_async,
                    &in_app_path,
                )?;
                let responses =
                    request_presigned_urls(&cfg, &vec![req], &api_key, bearer_header).await?;
                let upload_resp = responses
                    .into_iter()
                    .next()
                    .ok_or_else(|| "missing presigned response".to_string())?;

                if let Err(e) = upload_file_to_presigned(
                    &file_info.upload_path,
                    &upload_resp,
                    http.as_ref(),
                )
                .await
                {
                    let _ = progress_handle.add_error(e.clone());
                    progress_handle.set_upload_current(None::<&str>);
                    return Err(e);
                }

                if let Err(e) = uploads_tracking::record_upload(
                    &user_id,
                    &file_info.upload_path,
                    &in_app_path_str,
                    &upload_resp.asset_id,
                    &upload_request_id,
                ) {
                    let _ = progress_handle.add_warning(format!(
                        "Failed to record upload in tracking file: {}",
                        e
                    ));
                }
                completed_responses.push(upload_resp);
                progress_handle.set_upload_current(None::<&str>);
            }

            if !completed_responses.is_empty() {
                let mut preproc_req = ProcessAssetsRequest::new(
                    completed_responses.clone(),
                    None::<tellers_api_client::models::VersionReference>,
                );
                preproc_req.generate_time_based_media_description =
                    Some(!disable_description_generation);
                let _ = progress_handle.add_info("Triggering preprocessing...");
                let preproc_tasks = api::process_assets_users_assets_preprocess_post(
                    &cfg,
                    preproc_req,
                    None,
                    Some(&api_key),
                    bearer_header,
                )
                .await
                .map_err(|e| format!("failed to trigger preprocess: {}", e))?;
                let _ = progress_handle.add_success(format!(
                    "Preprocess tasks queued: {}",
                    preproc_tasks.len()
                ));
            }

            Ok(())
        };

        let ((), ()) = tokio::try_join!(producer, consumer)?;
        Ok::<_, String>((render_handle, progress))
    });

    let (render_handle, mut progress) = block_result?;
    rt.block_on(TwoQueueProgress::stop_render_loop(render_handle));
    progress.finish()?;
    println!();

    Ok(())
}

fn build_single_upload_request(
    file_info: &FileToUpload,
    base_dir: &PathBuf,
    in_app_path: &Option<String>,
) -> Result<(AssetUploadRequest, String, String), String> {
    let content_length = std::fs::metadata(&file_info.upload_path)
        .map_err(|e| format!("failed to stat {}: {}", file_info.upload_path.display(), e))?
        .len();
    let upload_id = Uuid::new_v4().to_string();
    let file_in_app_path =
        super::utils::compute_in_app_path(&file_info.original_path, base_dir, in_app_path);
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
    let umid = extract_media_metadata(&file_info.original_path).ok();
    let mut source_info = SourceFileInfo::new(
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
    if let Some(metadata) = umid {
        if let Some(umid_value) = metadata.material_package_umid {
            source_info.capture_device_umid = Some(Some(umid_value));
        }
        if let Some(first_umid) = metadata.file_package_umids.first() {
            source_info.umid = Some(Some(first_umid.clone()));
        }
    }
    let req = AssetUploadRequest::new(
        i32::try_from(content_length).unwrap_or(i32::MAX),
        upload_id.clone(),
        source_info,
    );
    Ok((req, upload_id, file_in_app_path))
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
            "headers: x-api-key=set, authorization={}",
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
                    .first()
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

async fn upload_file_to_presigned(
    file_path: &PathBuf,
    upload_resp: &AssetUploadResponse,
    http: &reqwest::Client,
) -> Result<(), String> {
    let total_bytes = std::fs::metadata(file_path)
        .map_err(|e| format!("failed to stat {}: {}", file_path.display(), e))?
        .len();
    let upload_url = upload_resp.presigned_put_url.clone();

    let mut f = File::open(file_path)
        .map_err(|e| format!("failed to open {}: {}", file_path.display(), e))?;
    let mut buf = Vec::with_capacity(total_bytes as usize);
    let mut chunk = vec![0u8; (1024 * 1024).min(total_bytes as usize)];
    loop {
        let n = f
            .read(&mut chunk)
            .map_err(|e| format!("failed to read {}: {}", file_path.display(), e))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
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

    if !put_res.status().is_success() {
        let status = put_res.status();
        let body = put_res
            .text()
            .await
            .unwrap_or_else(|_| "<failed to read error body>".to_string());
        return Err(format!(
            "Upload failed for {}: HTTP {} - {}",
            file_path.display(),
            status,
            body
        ));
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
        let n = f
            .read(&mut chunk)
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
        let _ =
            progress_handle.add_warning(format!("Failed to record upload in tracking file: {}", e));
    }

    Ok(())
}
