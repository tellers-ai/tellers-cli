use clap::{Args, Subcommand};
use regex::Regex;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::sleep;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::auth;
use crate::commands::api_config;
use crate::media::ffmpeg::ensure_ffmpeg_available;
use crate::media::media_file_type::{is_audio_file, is_image_file};
use crate::media::metadata::{extract_media_metadata, get_ffprobe_json};
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
use tellers_api_client::models::process_assets_request::GenerateProxy;
use tellers_api_client::models::{
    AssetUploadRequest, AssetUploadResponse, CreateFolderRequest, FileType, ProcessAssetsRequest,
    SourceFileInfo,
};

#[derive(Args, Debug)]
pub struct UploadArgs {
    #[command(subcommand)]
    pub command: UploadCommand,
}

#[derive(Subcommand, Debug)]
pub enum UploadCommand {
    /// Upload files from a path
    Upload(UploadCmdArgs),
    /// Recreate filesystem from a path
    RecreateFilesystem(RecreateFilesystemArgs),
}

#[derive(Args, Debug, Clone)]
pub struct RecreateFilesystemArgs {
    #[arg(long)]
    pub in_app_path: Option<String>,

    #[arg(long, default_value_t = false)]
    pub dry_run: bool,

    pub path: String,
}

#[derive(Args, Debug, Clone)]
pub struct UploadCmdArgs {
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

    /// Proxy heights to request from the server after upload (e.g. 720, 1080). Omitted by default to match tellers-app; local encoding uses --qualities.
    #[arg(long, value_delimiter = ',', num_args = 0.., value_parser = parse_generate_proxy)]
    pub generate_proxy: Option<Vec<GenerateProxy>>,

    #[arg(long, default_value_t = false)]
    pub disable_description_generation: bool,

    #[arg(long, default_value_t = false)]
    pub show_status_until_done: bool,

    #[arg(long, default_value_t = false)]
    pub show_status_until_analysed: bool,

    #[arg(long, default_value_t = false)]
    pub show_status_until_transcoded: bool,

    #[arg(long, default_value_t = false)]
    pub machine_readable: bool,
}

#[derive(Clone)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusWaitMode {
    Done,
    Analysed,
    Transcoded,
}

#[derive(Clone, Debug, Default)]
struct AssetTaskProgress {
    analyze_asset: Option<(String, f64)>,
    downscaling: Option<(String, f64)>,
    deep_analyze: Option<(String, f64)>,
}

#[derive(Clone, Debug)]
struct StatusWaitOutcome {
    success: bool,
    error: Option<String>,
    assets: Vec<MachineAssetStatus>,
}

#[derive(Clone, Debug)]
struct UploadedAssetInfo {
    asset_id: String,
    local_path: String,
}

#[derive(Clone, Debug)]
struct MachineAssetStatus {
    asset_id: String,
    local_path: String,
    status: String,
}

fn parse_generate_proxy(s: &str) -> Result<GenerateProxy, String> {
    match s.trim() {
        "360" => Ok(GenerateProxy::Variant360),
        "480" => Ok(GenerateProxy::Variant480),
        "720" => Ok(GenerateProxy::Variant720),
        "1080" => Ok(GenerateProxy::Variant1080),
        "2160" => Ok(GenerateProxy::Variant2160),
        _ => Err(format!(
            "generate_proxy must be one of 360, 480, 720, 1080, 2160, got '{}'",
            s
        )),
    }
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
    match args.command {
        UploadCommand::Upload(cmd_args) => run_upload(cmd_args),
        UploadCommand::RecreateFilesystem(cmd_args) => run_recreate_filesystem(cmd_args),
    }
}

fn run_recreate_filesystem(args: RecreateFilesystemArgs) -> Result<(), String> {
    let base_dir = PathBuf::from(&args.path);
    if !base_dir.exists() {
        return Err(format!("path not found: {}", base_dir.display()));
    }
    if base_dir.is_file() {
        return Err("path must be a directory".to_string());
    }

    // Collect all directories (root and every subfolder), with relative path for in-app mapping.
    // By default exclude any path that has a segment starting with '.' (e.g. .git, .fingerprint).
    let base_dir = base_dir.canonicalize().map_err(|e| format!("{}", e))?;
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(&base_dir).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_dir() {
            let p = entry.path();
            let rel = match p.strip_prefix(&base_dir) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let has_dot_component = rel.components().any(|c| {
                c.as_os_str()
                    .to_str()
                    .map(|s| s.starts_with('.'))
                    .unwrap_or(false)
            });
            if !has_dot_component {
                dirs.push(p.to_path_buf());
            }
        }
    }

    // Build in-app path for each dir: in_app_path + relative_path, or relative path only
    let mut in_app_paths: Vec<String> = dirs
        .iter()
        .map(|p| {
            let rel = p
                .strip_prefix(&base_dir)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/");
            let rel_trimmed = rel.trim_start_matches('/');
            match &args.in_app_path {
                Some(prefix) => {
                    let prefix = prefix.trim_end_matches('/');
                    if rel_trimmed.is_empty() {
                        prefix.to_string()
                    } else {
                        format!("{}/{}", prefix, rel_trimmed)
                    }
                }
                None => {
                    if rel_trimmed.is_empty() {
                        ".".to_string()
                    } else {
                        rel_trimmed.to_string()
                    }
                }
            }
        })
        .collect();

    // Don't create a folder named "." on the server when root maps to it
    in_app_paths.retain(|p| p != ".");

    // Sort by path depth so parents are created before children
    in_app_paths.sort_by_key(|p| p.matches('/').count());

    if args.dry_run {
        for p in &in_app_paths {
            output::info(format!("[dry run] would create folder: {}", p));
        }
        return Ok(());
    }

    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(None)?;
    let bearer_header = api_config::get_bearer_header(None);

    let rt =
        tokio::runtime::Runtime::new().map_err(|e| format!("failed to start runtime: {}", e))?;

    for folder_path in in_app_paths {
        let mut req = CreateFolderRequest::new();
        req.path = Some(Some(folder_path.clone()));
        let response = rt
            .block_on(api::create_folder_asset_folder_post(
                &cfg,
                req,
                Some(api_key.as_str()),
                bearer_header.as_deref(),
            ))
            .map_err(|e| e.to_string())?;
        println!("{}", response.path);
    }
    Ok(())
}

fn run_upload(args: UploadCmdArgs) -> Result<(), String> {
    let script_start = std::time::Instant::now();
    let active_status_flags = [
        args.show_status_until_done,
        args.show_status_until_analysed,
        args.show_status_until_transcoded,
    ]
    .into_iter()
    .filter(|v| *v)
    .count();
    if active_status_flags > 1 {
        return Err(
            "Use only one of --show-status-until-done, --show-status-until-analysed, --show-status-until-transcoded"
                .to_string(),
        );
    }
    let status_wait_mode = if args.show_status_until_done {
        Some(StatusWaitMode::Done)
    } else if args.show_status_until_analysed {
        Some(StatusWaitMode::Analysed)
    } else if args.show_status_until_transcoded {
        Some(StatusWaitMode::Transcoded)
    } else {
        None
    };
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
        let regex_patterns = regex_patterns.map_err(|e| format!("invalid regex pattern: {}", e))?;

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
            args.disable_description_generation,
        );
    }

    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(None)?;
    if !args.machine_readable {
        output::info(format!("API base: {}", cfg.base_path));
    }

    let bearer_header_for_auth = api_config::get_bearer_header(args.auth_bearer.clone());
    let user_id = auth::get_user_id_from_bearer_with_logging(
        bearer_header_for_auth.as_deref(),
        !args.machine_readable,
    );

    if !args.force_upload {
        let before = original_files.len();
        original_files.retain(|path| {
            !super::utils::is_already_uploaded(path, &user_id, &base_dir, &args.in_app_path)
        });
        let skipped = before - original_files.len();
        if skipped > 0 && !args.machine_readable {
            output::info(format!("Skipped {} already uploaded file(s)", skipped));
        }
        if original_files.is_empty() {
            return Err("no files to upload (all files were already uploaded)".to_string());
        }
    }

    let upload_request_id = Uuid::new_v4().to_string();

    if args.local_encoding {
        ensure_ffmpeg_available()?;
        if !args.machine_readable {
            output::info(format!(
                "Local encoding: downscale + upload queues ({} quality)",
                args.qualities
                    .iter()
                    .map(|q| q.as_label())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if args.qualities.len() > 1 {
            return Err("Only supporting single quality for now".to_string());
        }
        let work_items: Vec<DownscaleWork> = build_downscale_work(&original_files)?;
        if !args.machine_readable {
            output::info(format!("{} file(s) in downscale queue", work_items.len()));
        }
        let uploaded_asset_ids = run_two_queue_pipeline(
            work_items,
            &base_dir,
            &args,
            &cfg,
            &api_key,
            bearer_header_for_auth.as_deref(),
            &user_id,
            &upload_request_id,
        );
        let uploaded_asset_ids = uploaded_asset_ids?;
        if let Some(mode) = status_wait_mode {
            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| format!("failed to start runtime: {}", e))?;
            let wait_result = rt.block_on(wait_for_asset_processing_status(
                &cfg,
                &api_key,
                bearer_header_for_auth.as_deref(),
                &uploaded_asset_ids,
                mode,
                script_start,
                args.machine_readable,
            ));
            match wait_result {
                Ok(outcome) => {
                    if args.machine_readable {
                        print_machine_readable_result(
                            &outcome,
                            Some(script_start.elapsed().as_secs()),
                        );
                    }
                    if !outcome.success {
                        return Err(outcome.error.unwrap_or_else(|| {
                            "one or more assets failed watched tasks".to_string()
                        }));
                    }
                }
                Err(e) => {
                    if args.machine_readable {
                        let failure_outcome = StatusWaitOutcome {
                            success: false,
                            error: Some(e.clone()),
                            assets: uploaded_asset_ids
                                .iter()
                                .map(|a| MachineAssetStatus {
                                    asset_id: a.asset_id.clone(),
                                    local_path: a.local_path.clone(),
                                    status: "unknown".to_string(),
                                })
                                .collect(),
                        };
                        print_machine_readable_result(&failure_outcome, None);
                    }
                    return Err(e);
                }
            }
        }
        return Ok(());
    }

    let mut files_to_upload: Vec<FileToUpload> = Vec::new();
    if !args.machine_readable {
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
    }
    for original_file in original_files {
        files_to_upload.push(FileToUpload {
            upload_path: original_file.clone(),
            original_path: original_file,
        });
    }

    let effective_generate_proxy = args.generate_proxy.clone();

    let base_dir = base_dir.clone();
    let in_app_path = args.in_app_path.clone();

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            let bearer_header = api_config::get_bearer_header(args.auth_bearer.clone());
            let mut progress = if args.machine_readable {
                None
            } else {
                Some(crate::tui::InlineProgress::new(
                    "Uploading Files",
                    files_to_upload.len(),
                )?)
            };
            let progress_handle = progress.as_ref().map(|p| p.clone_handle());
            let render_handle =
                if let (Some(p), Some(ph)) = (progress.as_mut(), progress_handle.as_ref()) {
                    Some(p.start_render_loop(ph.clone()))
                } else {
                    None
                };

            if let Some(ph) = progress_handle.as_ref() {
                let _ = ph.add_info(format!(
                    "One presigned request per file ({} files), {} parallel upload(s)",
                    files_to_upload.len(),
                    args.parallel_uploads
                ));
            }

            let upload_result = upload_with_per_file_presigned(
                &files_to_upload,
                &base_dir,
                &in_app_path,
                &upload_request_id,
                &user_id,
                args.parallel_uploads,
                progress_handle.as_ref(),
                &cfg,
                &api_key,
                bearer_header.as_deref(),
                args.disable_description_generation,
                effective_generate_proxy.as_ref(),
            )
            .await;

            if let (Err(e), Some(ph)) = (&upload_result, progress_handle.as_ref()) {
                let _ = ph.add_error(format!("Upload failed: {}", e));
            }
            let uploaded_asset_ids = upload_result?;

            if let Some(ph) = progress_handle.as_ref() {
                let _ = ph.add_success("All uploads completed");
            }

            if let Some(handle) = render_handle {
                crate::tui::InlineProgress::stop_render_loop(handle).await;
            }
            if let Some(mut p) = progress {
                p.finish()?;
                // Add empty line after progress display
                println!();
            }

            if let Some(mode) = status_wait_mode {
                let wait_result = wait_for_asset_processing_status(
                    &cfg,
                    &api_key,
                    bearer_header.as_deref(),
                    &uploaded_asset_ids,
                    mode,
                    script_start,
                    args.machine_readable,
                )
                .await;
                match wait_result {
                    Ok(outcome) => {
                        if args.machine_readable {
                            print_machine_readable_result(
                                &outcome,
                                Some(script_start.elapsed().as_secs()),
                            );
                        }
                        if !outcome.success {
                            return Err(outcome.error.unwrap_or_else(|| {
                                "one or more assets failed watched tasks".to_string()
                            }));
                        }
                    }
                    Err(e) => {
                        if args.machine_readable {
                            let failure_outcome = StatusWaitOutcome {
                                success: false,
                                error: Some(e.clone()),
                                assets: uploaded_asset_ids
                                    .iter()
                                    .map(|a| MachineAssetStatus {
                                        asset_id: a.asset_id.clone(),
                                        local_path: a.local_path.clone(),
                                        status: "unknown".to_string(),
                                    })
                                    .collect(),
                            };
                            print_machine_readable_result(&failure_outcome, None);
                        }
                        return Err(e);
                    }
                }
            }

            Ok(())
        })
}

fn work_item_file_name(w: &DownscaleWork) -> String {
    let p = match w {
        DownscaleWork::MxfVideo(p)
        | DownscaleWork::MxfAudio(p)
        | DownscaleWork::Video(p)
        | DownscaleWork::Audio(p)
        | DownscaleWork::Passthrough(p) => p,
    };
    p.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
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
            let mut progress_cb = |pct: f64| progress_handle.set_downscale_current_pct(Some(pct));
            let info_cb = |msg: &str| progress_handle.add_info(msg);
            match create_rendition(&original_path, def, Some(&mut progress_cb), Some(&info_cb)) {
                Ok(upload_path) => FileToUpload {
                    upload_path,
                    original_path,
                },
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
        DownscaleWork::MxfAudio(original_path) => {
            let mut progress_cb = |pct: f64| progress_handle.set_downscale_current_pct(Some(pct));
            let info_cb = |msg: &str| progress_handle.add_info(msg);
            match convert_to_mp3(&original_path, None, Some(&mut progress_cb), Some(&info_cb)) {
                Ok(upload_path) => FileToUpload {
                    upload_path,
                    original_path,
                },
                Err(e) => {
                    let _ = progress_handle.add_error(format!(
                        "MXF to MP3 failed for {}: {}",
                        original_path.display(),
                        e
                    ));
                    return Ok(None);
                }
            }
        }
        DownscaleWork::Video(original_path) => {
            let def = RenditionDefinition {
                quality: Some(qualities[0]),
                preset,
                crf: None,
                audio_bitrate: None,
            };
            let mut progress_cb = |pct: f64| progress_handle.set_downscale_current_pct(Some(pct));
            let info_cb = |msg: &str| progress_handle.add_info(msg);
            match create_rendition(&original_path, def, Some(&mut progress_cb), Some(&info_cb)) {
                Ok(upload_path) => FileToUpload {
                    upload_path,
                    original_path,
                },
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
        DownscaleWork::Audio(original_path) => {
            let mut progress_cb = |pct: f64| progress_handle.set_downscale_current_pct(Some(pct));
            let info_cb = |msg: &str| progress_handle.add_info(msg);
            match normalize_audio_to_mp3(
                &original_path,
                Some(192),
                Some(&mut progress_cb),
                Some(&info_cb),
            ) {
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
            }
        }
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
    args: &UploadCmdArgs,
    cfg: &Configuration,
    api_key: &str,
    bearer_opt: Option<&str>,
    user_id: &str,
    upload_request_id: &str,
) -> Result<Vec<UploadedAssetInfo>, String> {
    let (upload_tx, mut upload_rx) = tokio_mpsc::channel::<FileToUpload>(64);

    let mut progress = TwoQueueProgress::new()?;
    let progress_handle = progress.clone_handle();
    progress_handle.set_downscale_queued(work_items.len());
    let downscale_pending_names: Vec<String> = work_items.iter().map(work_item_file_name).collect();
    progress_handle.set_downscale_pending(downscale_pending_names);

    let rt =
        tokio::runtime::Runtime::new().map_err(|e| format!("failed to start runtime: {}", e))?;

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
    let generate_proxy = args.generate_proxy.clone();

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
                let file =
                    tokio::task::spawn_blocking(move || do_one_downscale(w, &ph, &qual, preset))
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
                    upload_tx_producer
                        .send(f)
                        .await
                        .map_err(|_| "upload channel closed".to_string())?;
                }
            }
            drop(upload_tx_producer);
            Ok::<(), String>(())
        };
        drop(upload_tx);

        let uploaded_asset_ids: Arc<std::sync::Mutex<Vec<UploadedAssetInfo>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let uploaded_asset_ids_consumer = Arc::clone(&uploaded_asset_ids);
        let consumer = async move {
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

                let (req, _upload_id, in_app_path_str) =
                    build_single_upload_request(&file_info, &base_dir_async, &in_app_path)?;
                let responses =
                    request_presigned_urls(&cfg, &vec![req], &api_key, bearer_header).await?;
                let upload_resp = responses
                    .into_iter()
                    .next()
                    .ok_or_else(|| "missing presigned response".to_string())?;

                progress_handle.set_upload_current_pct(Some(0.0));
                if let Err(e) = upload_file_to_presigned(
                    &file_info.upload_path,
                    &upload_resp,
                    http.as_ref(),
                    &cfg,
                    &api_key,
                    bearer_header,
                    Some(&progress_handle),
                )
                .await
                {
                    let _ = progress_handle.add_error(e.clone());
                    progress_handle.set_upload_current(None::<&str>);
                    progress_handle.set_upload_current_pct(None);
                    return Err(e);
                }

                progress_handle.set_upload_current_pct(None);
                if let Err(e) = uploads_tracking::record_upload(
                    &user_id,
                    file_info.upload_path.as_path(),
                    &in_app_path_str,
                    &upload_resp.asset_id,
                    &upload_request_id,
                ) {
                    let _ = progress_handle
                        .add_warning(format!("Failed to record upload in tracking file: {}", e));
                }
                if let Ok(mut guard) = uploaded_asset_ids_consumer.lock() {
                    guard.push(UploadedAssetInfo {
                        asset_id: upload_resp.asset_id.clone(),
                        local_path: file_info.original_path.to_string_lossy().to_string(),
                    });
                }
                // Call preprocess as soon as this upload finishes
                let _ = progress_handle.add_info("Triggering preprocessing...");
                let mut preproc_req = ProcessAssetsRequest::new(
                    vec![upload_resp.clone()],
                    None::<tellers_api_client::models::VersionReference>,
                );
                preproc_req.cutter_sensitivity = Some(0.2);
                preproc_req.generate_time_based_media_description =
                    Some(!disable_description_generation);
                preproc_req.generate_proxy = generate_proxy.clone();
                let preproc_tasks = api::process_assets_users_assets_preprocess_post(
                    &cfg,
                    preproc_req,
                    None,
                    Some(&api_key),
                )
                .await
                .map_err(|e| format!("failed to trigger preprocess: {}", e))?;
                let _ = progress_handle
                    .add_success(format!("Preprocess tasks queued: {}", preproc_tasks.len()));

                progress_handle.set_upload_current(None::<&str>);
                progress_handle.set_upload_current_pct(None);
            }

            Ok(())
        };

        let join_result = tokio::try_join!(producer, consumer);
        let collected_ids = uploaded_asset_ids
            .lock()
            .map(|ids| ids.clone())
            .unwrap_or_default();
        Ok::<_, String>((render_handle, progress, join_result, collected_ids))
    });

    let (render_handle, mut progress, join_result, collected_ids) = block_result?;
    rt.block_on(TwoQueueProgress::stop_render_loop(render_handle));
    let _ = progress.finish();
    progress.print_messages_to_stderr();
    println!();

    join_result?;
    Ok(collected_ids)
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
        if let Some(first_with_data) = metadata.file_package_umids.iter().find(|u| u.has_data) {
            source_info.umid = Some(Some(first_with_data.umid.clone()));
        }
    }
    if is_mxf_file(&file_info.original_path) {
        if let Ok(Some(probe)) = get_ffprobe_json(&file_info.original_path) {
            if let serde_json::Value::Object(map) = probe {
                source_info.original_ffprobe_metadata = Some(Some(map.into_iter().collect()));
            }
        }
    }
    let mut req = AssetUploadRequest::new(
        i32::try_from(content_length).unwrap_or(i32::MAX),
        upload_id.clone(),
        source_info,
    );
    req.file_type = Some(infer_file_type(&file_info.original_path));
    Ok((req, upload_id, file_in_app_path))
}

fn infer_file_type(path: &PathBuf) -> FileType {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if ext == "aaf" {
        FileType::Aaf
    } else if is_image_file(path) {
        FileType::Image
    } else if is_audio_file(path) {
        FileType::Audio
    } else {
        FileType::Video
    }
}

const UPLOAD_URLS_MAX_RETRIES: u32 = 3;
const UPLOAD_URLS_RETRY_DELAY: Duration = Duration::from_secs(3);

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

    let mut last_err = None;
    for attempt in 1..=UPLOAD_URLS_MAX_RETRIES {
        match api::create_upload_urls_users_assets_upload_urls_post(
            cfg,
            requests.clone(),
            Some(api_key),
            bearer_opt,
        )
        .await
        {
            Ok(responses) => return Ok(responses),
            Err(e) => {
                let retryable = match &e {
                    tellers_api_client::apis::Error::ResponseError(resp) => {
                        let code = resp.status.as_u16();
                        code == 502 || code == 503 || code == 504
                    }
                    tellers_api_client::apis::Error::Reqwest(req_err) => {
                        req_err.is_timeout() || req_err.is_connect()
                    }
                    _ => false,
                };
                last_err = Some(e);
                if retryable && attempt < UPLOAD_URLS_MAX_RETRIES {
                    output::info(format!(
                        "Upload URL request failed (attempt {}/{}), retrying in {}s...",
                        attempt,
                        UPLOAD_URLS_MAX_RETRIES,
                        UPLOAD_URLS_RETRY_DELAY.as_secs()
                    ));
                    sleep(UPLOAD_URLS_RETRY_DELAY).await;
                } else {
                    break;
                }
            }
        }
    }

    let e = last_err.unwrap();
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
    Err(m)
}

/// Upload files with one presigned URL request per file (queue-style, like encode local).
async fn upload_with_per_file_presigned(
    files_to_upload: &[FileToUpload],
    base_dir: &PathBuf,
    in_app_path: &Option<String>,
    upload_request_id: &str,
    user_id: &str,
    max_concurrent: usize,
    progress_handle: Option<&ProgressHandle>,
    cfg: &Configuration,
    api_key: &str,
    bearer_opt: Option<&str>,
    disable_description_generation: bool,
    generate_proxy: Option<&Vec<GenerateProxy>>,
) -> Result<Vec<UploadedAssetInfo>, String> {
    let http = Arc::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| format!("failed to build http client: {}", e))?,
    );

    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let mut upload_tasks = Vec::new();
    let uploaded_asset_ids: Arc<std::sync::Mutex<Vec<UploadedAssetInfo>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let cfg = cfg.clone();
    let api_key = api_key.to_string();
    let bearer_opt = bearer_opt.map(String::from);
    let base_dir = base_dir.clone();
    let in_app_path = in_app_path.clone();
    let upload_request_id = upload_request_id.to_string();
    let user_id = user_id.to_string();
    let generate_proxy = generate_proxy.cloned();

    for (i, file_info) in files_to_upload.iter().enumerate() {
        let file_info = file_info.clone();
        let http_clone = Arc::clone(&http);
        let semaphore_clone = Arc::clone(&semaphore);
        let progress_handle_clone = progress_handle.cloned();
        let cfg_clone = cfg.clone();
        let api_key_clone = api_key.clone();
        let bearer_clone = bearer_opt.clone();
        let base_dir_clone = base_dir.clone();
        let in_app_path_clone = in_app_path.clone();
        let upload_request_id_clone = upload_request_id.clone();
        let user_id_clone = user_id.clone();
        let generate_proxy_clone = generate_proxy.clone();
        let uploaded_asset_ids_clone = Arc::clone(&uploaded_asset_ids);
        let task_id = i;

        let task = tokio::spawn(async move {
            let _permit = semaphore_clone
                .acquire()
                .await
                .map_err(|e| format!("failed to acquire semaphore: {}", e))?;

            let (req, _upload_id, in_app_path_str) =
                build_single_upload_request(&file_info, &base_dir_clone, &in_app_path_clone)?;

            let file_size = req.content_length.max(0) as u64;

            let responses = request_presigned_urls(
                &cfg_clone,
                &vec![req],
                &api_key_clone,
                bearer_clone.as_deref(),
            )
            .await?;
            let upload_resp = responses
                .into_iter()
                .next()
                .ok_or_else(|| "missing presigned response".to_string())?;

            let file_name = file_info
                .upload_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if let Some(ph) = progress_handle_clone.as_ref() {
                let _ = ph.start_task(task_id, file_name.clone(), file_size);
            }

            let result = upload_single_file(
                &file_info.upload_path,
                &upload_resp.upload_id,
                &upload_resp,
                &in_app_path_str,
                &upload_request_id_clone,
                &user_id_clone,
                task_id,
                http_clone.as_ref(),
                progress_handle_clone.as_ref(),
                file_size,
                &cfg_clone,
                &api_key_clone,
                bearer_clone.as_deref(),
            )
            .await;

            let success = result.is_ok();
            if let Some(ph) = progress_handle_clone.as_ref() {
                let _ = ph.finish_task(task_id, success);
            }

            if success {
                if let Ok(mut guard) = uploaded_asset_ids_clone.lock() {
                    guard.push(UploadedAssetInfo {
                        asset_id: upload_resp.asset_id.clone(),
                        local_path: file_info.original_path.to_string_lossy().to_string(),
                    });
                }
                let mut preproc_req = ProcessAssetsRequest::new(
                    vec![upload_resp],
                    None::<tellers_api_client::models::VersionReference>,
                );
                preproc_req.cutter_sensitivity = Some(0.2);
                preproc_req.generate_time_based_media_description =
                    Some(!disable_description_generation);
                if let Some(ref proxy) = generate_proxy_clone {
                    preproc_req.generate_proxy = Some(proxy.clone());
                }
                if let Err(e) = api::process_assets_users_assets_preprocess_post(
                    &cfg_clone,
                    preproc_req,
                    None,
                    Some(&api_key_clone),
                )
                .await
                {
                    if let Some(ph) = progress_handle_clone.as_ref() {
                        let _ = ph.add_error(format!("Failed to trigger preprocess: {}", e));
                    }
                    return Err(format!("preprocess: {}", e));
                }
            }

            result
        });

        upload_tasks.push(task);
    }

    for task in upload_tasks {
        task.await
            .map_err(|e| format!("upload task panicked: {}", e))?
            .map_err(|e| format!("upload failed: {}", e))?;
    }

    Ok(uploaded_asset_ids
        .lock()
        .map(|ids| ids.clone())
        .unwrap_or_default())
}

fn normalize_task_type(task_type: &str) -> Option<&'static str> {
    let t = task_type.trim().to_ascii_lowercase();
    if t == "analyze asset" {
        Some("analyze asset")
    } else if t == "downscaling" {
        Some("downscaling")
    } else if t == "deep analyze" {
        Some("deep analyze")
    } else {
        None
    }
}

fn is_terminal_status(status: &str) -> bool {
    let s = status.trim().to_ascii_lowercase();
    matches!(
        s.as_str(),
        "success" | "succeeded" | "done" | "completed" | "error" | "failed" | "cancelled"
    )
}

fn needs_task_for_mode(mode: StatusWaitMode, task_type: &str) -> bool {
    match mode {
        StatusWaitMode::Done => {
            matches!(task_type, "analyze asset" | "downscaling" | "deep analyze")
        }
        StatusWaitMode::Analysed => matches!(task_type, "analyze asset" | "deep analyze"),
        StatusWaitMode::Transcoded => task_type == "downscaling",
    }
}

fn all_done_for_mode(mode: StatusWaitMode, progress: &AssetTaskProgress) -> bool {
    let check = |entry: &Option<(String, f64)>| -> bool {
        entry
            .as_ref()
            .map(|(status, _)| is_terminal_status(status))
            .unwrap_or(false)
    };
    match mode {
        StatusWaitMode::Done => {
            check(&progress.analyze_asset)
                && check(&progress.downscaling)
                && check(&progress.deep_analyze)
        }
        StatusWaitMode::Analysed => check(&progress.analyze_asset) && check(&progress.deep_analyze),
        StatusWaitMode::Transcoded => check(&progress.downscaling),
    }
}

fn is_error_status(status: &str) -> bool {
    let s = status.trim().to_ascii_lowercase();
    matches!(s.as_str(), "error" | "failed")
}

fn has_error_for_mode(mode: StatusWaitMode, progress: &AssetTaskProgress) -> bool {
    let has_error = |entry: &Option<(String, f64)>| -> bool {
        entry
            .as_ref()
            .map(|(status, _)| is_error_status(status))
            .unwrap_or(false)
    };
    match mode {
        StatusWaitMode::Done => {
            has_error(&progress.analyze_asset)
                || has_error(&progress.downscaling)
                || has_error(&progress.deep_analyze)
        }
        StatusWaitMode::Analysed => {
            has_error(&progress.analyze_asset) || has_error(&progress.deep_analyze)
        }
        StatusWaitMode::Transcoded => has_error(&progress.downscaling),
    }
}

fn asset_done_for_mode(mode: StatusWaitMode, progress: &AssetTaskProgress) -> bool {
    // If one watched task errors for this asset, stop waiting for other tasks of this asset.
    has_error_for_mode(mode, progress) || all_done_for_mode(mode, progress)
}

fn task_progress_to_percent(progress: f64) -> f64 {
    if (0.0..=1.0).contains(&progress) {
        progress * 100.0
    } else {
        progress.clamp(0.0, 100.0)
    }
}

fn render_status_row(asset_id: &str, progress: &AssetTaskProgress) -> String {
    let analyze = progress
        .analyze_asset
        .as_ref()
        .map(|(s, p)| format!("{}:{:.0}%", s, task_progress_to_percent(*p)))
        .unwrap_or_else(|| "pending".to_string());
    let downscaling = progress
        .downscaling
        .as_ref()
        .map(|(s, p)| format!("{}:{:.0}%", s, task_progress_to_percent(*p)))
        .unwrap_or_else(|| "pending".to_string());
    let deep_analyze = progress
        .deep_analyze
        .as_ref()
        .map(|(s, p)| format!("{}:{:.0}%", s, task_progress_to_percent(*p)))
        .unwrap_or_else(|| "pending".to_string());
    let asset_display = if asset_id.len() > 20 {
        format!(
            "{}...{}",
            &asset_id[..8],
            &asset_id[asset_id.len().saturating_sub(8)..]
        )
    } else {
        asset_id.to_string()
    };
    format!(
        "asset_id={} | analyze asset={} | downscaling={} | deep analyze={}",
        asset_display, analyze, downscaling, deep_analyze
    )
}

fn render_status_rows(
    ordered_asset_ids: &[String],
    progress_by_asset: &HashMap<String, AssetTaskProgress>,
) -> Vec<String> {
    let mut rows = Vec::with_capacity(ordered_asset_ids.len());
    for asset_id in ordered_asset_ids {
        if let Some(progress) = progress_by_asset.get(asset_id) {
            rows.push(render_status_row(asset_id, progress));
        }
    }
    rows
}

fn print_machine_readable_result(outcome: &StatusWaitOutcome, elapsed_seconds: Option<u64>) {
    let assets_json: Vec<serde_json::Value> = outcome
        .assets
        .iter()
        .map(|a| {
            serde_json::json!({
                "local_path": a.local_path,
                "asset_id": a.asset_id,
                "status": a.status,
            })
        })
        .collect();
    let payload = if outcome.success {
        serde_json::json!({
            "success": true,
            "assets": assets_json,
            "elapsed_seconds": elapsed_seconds.unwrap_or_default(),
        })
    } else {
        serde_json::json!({
            "success": false,
            "assets": assets_json,
            "error": outcome
                .error
                .clone()
                .unwrap_or_else(|| "one or more assets failed watched tasks".to_string()),
        })
    };
    println!("{}", payload);
}

async fn wait_for_asset_processing_status(
    cfg: &Configuration,
    api_key: &str,
    bearer_opt: Option<&str>,
    uploaded_assets: &[UploadedAssetInfo],
    mode: StatusWaitMode,
    script_start: std::time::Instant,
    machine_readable: bool,
) -> Result<StatusWaitOutcome, String> {
    if uploaded_assets.is_empty() {
        if !machine_readable {
            output::info("No uploaded asset_id found; skipping task polling");
        }
        return Ok(StatusWaitOutcome {
            success: true,
            error: None,
            assets: Vec::new(),
        });
    }
    let mut progress_by_asset: HashMap<String, AssetTaskProgress> = uploaded_assets
        .iter()
        .map(|a| (a.asset_id.clone(), AssetTaskProgress::default()))
        .collect();
    let ordered_asset_ids: Vec<String> =
        uploaded_assets.iter().map(|a| a.asset_id.clone()).collect();

    if !machine_readable {
        output::info("Polling /users/tasks every 2s for processing status...");
    }
    let mut status_progress = if machine_readable {
        None
    } else {
        Some(crate::tui::InlineProgress::new(
            "Processing Uploaded Assets",
            uploaded_assets.len(),
        )?)
    };
    let status_handle = status_progress.as_ref().map(|p| p.clone_handle());
    if let Some(handle) = status_handle.as_ref() {
        let _ = handle.set_show_elapsed(false);
        for (task_id, asset) in uploaded_assets.iter().enumerate() {
            let _ = handle.start_task(task_id, asset.asset_id.clone(), 100);
        }
    }
    let status_render_handle = if let (Some(progress), Some(handle)) =
        (status_progress.as_mut(), status_handle.as_ref())
    {
        Some(progress.start_render_loop(handle.clone()))
    } else {
        None
    };
    loop {
        let finish_before_seconds = (script_start.elapsed().as_secs() as i32) + 60;
        let tasks = api::get_tasks_users_tasks_get(
            cfg,
            Some(finish_before_seconds),
            Some(api_key),
            bearer_opt,
        )
        .await
        .map_err(|e| format!("failed to get tasks: {}", e))?;

        for task in tasks {
            let Some(normalized_type) = normalize_task_type(&task.task_type) else {
                continue;
            };
            if !needs_task_for_mode(mode, normalized_type) {
                continue;
            }
            for asset_id in task.asset_ids {
                let Some(entry) = progress_by_asset.get_mut(&asset_id) else {
                    continue;
                };
                let pair = (task.status.clone(), task.progress);
                match normalized_type {
                    "analyze asset" => entry.analyze_asset = Some(pair),
                    "downscaling" => entry.downscaling = Some(pair),
                    "deep analyze" => entry.deep_analyze = Some(pair),
                    _ => {}
                }
            }
        }

        if let Some(handle) = status_handle.as_ref() {
            let rendered_rows = render_status_rows(&ordered_asset_ids, &progress_by_asset);
            for (task_id, asset_id) in ordered_asset_ids.iter().enumerate() {
                if let Some(asset_progress) = progress_by_asset.get(asset_id) {
                    let row = rendered_rows
                        .get(task_id)
                        .cloned()
                        .unwrap_or_else(|| render_status_row(asset_id, asset_progress));
                    let _ = handle.set_task_label(task_id, row);
                    let pct = match mode {
                        StatusWaitMode::Done => {
                            let mut sum = 0.0;
                            let mut count = 0.0;
                            if let Some((_, p)) = asset_progress.analyze_asset.as_ref() {
                                sum += task_progress_to_percent(*p);
                                count += 1.0;
                            }
                            if let Some((_, p)) = asset_progress.downscaling.as_ref() {
                                sum += task_progress_to_percent(*p);
                                count += 1.0;
                            }
                            if let Some((_, p)) = asset_progress.deep_analyze.as_ref() {
                                sum += task_progress_to_percent(*p);
                                count += 1.0;
                            }
                            if count > 0.0 {
                                sum / count
                            } else {
                                0.0
                            }
                        }
                        StatusWaitMode::Analysed => {
                            let mut sum = 0.0;
                            let mut count = 0.0;
                            if let Some((_, p)) = asset_progress.analyze_asset.as_ref() {
                                sum += task_progress_to_percent(*p);
                                count += 1.0;
                            }
                            if let Some((_, p)) = asset_progress.deep_analyze.as_ref() {
                                sum += task_progress_to_percent(*p);
                                count += 1.0;
                            }
                            if count > 0.0 {
                                sum / count
                            } else {
                                0.0
                            }
                        }
                        StatusWaitMode::Transcoded => asset_progress
                            .downscaling
                            .as_ref()
                            .map(|(_, p)| task_progress_to_percent(*p))
                            .unwrap_or(0.0),
                    };
                    let _ = handle.set_task_progress_pct(task_id, pct);
                }
            }
        }

        let all_done = ordered_asset_ids.iter().all(|asset_id| {
            progress_by_asset
                .get(asset_id)
                .map(|p| asset_done_for_mode(mode, p))
                .unwrap_or(false)
        });

        if all_done {
            let had_error = ordered_asset_ids.iter().any(|asset_id| {
                progress_by_asset
                    .get(asset_id)
                    .map(|p| has_error_for_mode(mode, p))
                    .unwrap_or(false)
            });
            if let Some(handle) = status_handle.as_ref() {
                for (task_id, asset_id) in ordered_asset_ids.iter().enumerate() {
                    let has_error = progress_by_asset
                        .get(asset_id)
                        .map(|p| has_error_for_mode(mode, p))
                        .unwrap_or(false);
                    let _ = handle.finish_task(task_id, !has_error);
                    if let Some(asset_progress) = progress_by_asset.get(asset_id) {
                        if let Some((status, _)) = asset_progress.downscaling.as_ref() {
                            if status.eq_ignore_ascii_case("error")
                                || status.eq_ignore_ascii_case("failed")
                            {
                                let _ = handle.add_warning(format!(
                                    "asset_id={} reached terminal status with downscaling={}",
                                    asset_id, status
                                ));
                            }
                        }
                    }
                }
            }
            if let Some(render_handle) = status_render_handle {
                crate::tui::InlineProgress::stop_render_loop(render_handle).await;
            }
            if let Some(progress) = status_progress.as_mut() {
                progress.finish()?;
                println!();
            }
            if !machine_readable {
                output::success("All watched assets reached terminal state for selected tasks");
            }
            let assets: Vec<MachineAssetStatus> = uploaded_assets
                .iter()
                .map(|asset| {
                    let status = progress_by_asset
                        .get(&asset.asset_id)
                        .map(|p| {
                            if has_error_for_mode(mode, p) {
                                "error"
                            } else {
                                "success"
                            }
                        })
                        .unwrap_or("unknown")
                        .to_string();
                    MachineAssetStatus {
                        asset_id: asset.asset_id.clone(),
                        local_path: asset.local_path.clone(),
                        status,
                    }
                })
                .collect();
            return Ok(StatusWaitOutcome {
                success: !had_error,
                error: if had_error {
                    Some("one or more assets failed watched tasks".to_string())
                } else {
                    None
                },
                assets,
            });
        }

        sleep(Duration::from_secs(2)).await;
    }
}

fn single_put_url(resp: &AssetUploadResponse) -> String {
    resp.presigned_put_url
        .clone()
        .flatten()
        .unwrap_or_else(String::new)
}

pub async fn upload_file_to_presigned(
    file_path: &PathBuf,
    upload_resp: &AssetUploadResponse,
    http: &reqwest::Client,
    _cfg: &Configuration,
    _api_key: &str,
    _bearer_opt: Option<&str>,
    progress_handle: Option<&TwoQueueProgressHandle>,
) -> Result<(), String> {
    let total_bytes = std::fs::metadata(file_path)
        .map_err(|e| format!("failed to stat {}: {}", file_path.display(), e))?
        .len();
    let upload_url = single_put_url(upload_resp);

    let mut f = File::open(file_path)
        .map_err(|e| format!("failed to open {}: {}", file_path.display(), e))?;
    let mut buf = Vec::with_capacity(total_bytes as usize);
    let mut chunk = vec![0u8; (1024 * 1024).min(total_bytes as usize)];
    let mut read_so_far = 0u64;
    loop {
        let n = f
            .read(&mut chunk)
            .map_err(|e| format!("failed to read {}: {}", file_path.display(), e))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        read_so_far += n as u64;
        if let Some(ph) = progress_handle {
            let pct = if total_bytes > 0 {
                100.0 * (read_so_far as f64 / total_bytes as f64)
            } else {
                100.0
            };
            ph.set_upload_current_pct(Some(pct));
        }
    }

    let content_type = mime_guess::from_path(file_path)
        .first_or_text_plain()
        .essence_str()
        .to_string();

    let put_res = http
        .put(upload_url.as_str())
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
    progress_handle: Option<&ProgressHandle>,
    total_bytes: u64,
    _cfg: &Configuration,
    _api_key: &str,
    _bearer_opt: Option<&str>,
) -> Result<(), String> {
    let upload_url = single_put_url(upload_resp);

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
        if let Some(ph) = progress_handle {
            let _ = ph.update_task(task_id, uploaded);
        }
    }

    let content_type = mime_guess::from_path(file_path)
        .first_or_text_plain()
        .essence_str()
        .to_string();

    let put_res = http
        .put(upload_url.as_str())
        .header(reqwest::header::CONTENT_LENGTH, total_bytes)
        .header(reqwest::header::CONTENT_TYPE, &content_type)
        .body(buf)
        .send()
        .await
        .map_err(|e| format!("upload failed for {}: {}", file_path.display(), e))?;

    if let Some(ph) = progress_handle {
        let _ = ph.update_task(task_id, total_bytes);
    }

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
        if let Some(ph) = progress_handle {
            let _ = ph.add_error(error_msg.clone());
        }
        return Err(error_msg);
    }

    if let Err(e) = uploads_tracking::record_upload(
        user_id,
        file_path.as_path(),
        in_app_path,
        &upload_resp.asset_id,
        upload_request_id,
    ) {
        if let Some(ph) = progress_handle {
            let _ = ph.add_warning(format!("Failed to record upload in tracking file: {}", e));
        }
    }

    Ok(())
}
