use std::path::PathBuf;
use std::time::Instant;

use crate::auth;
use crate::media::ffmpeg::get_media_duration;
use crate::media::media_file_type::{is_audio_file, is_image_file, is_metadata_file};
use crate::media::transcode::{has_video_streams, is_mxf_file};
use crate::media::video_file_ext::has_video_ext;
use crate::output;
use crate::tui::InlineProgress;

use super::utils::is_already_uploaded;

pub fn run_dry_run(
    original_files: &[PathBuf],
    base_dir: &PathBuf,
    in_app_path: &Option<String>,
    auth_bearer: &Option<String>,
    force_upload: bool,
) -> Result<(), String> {
    let bearer_env = auth_bearer
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

    let mut files_to_check: Vec<PathBuf> = original_files.to_vec();
    files_to_check.retain(|file_path| !is_metadata_file(file_path));
    if !force_upload {
        files_to_check
            .retain(|file_path| !is_already_uploaded(file_path, &user_id, base_dir, in_app_path));
    }

    if files_to_check.is_empty() {
        return Err("no files to upload (all files were already uploaded)".to_string());
    }

    output::info(format!(
        "Dry run: analyzing {} file(s)",
        files_to_check.len()
    ));

    let mut progress = InlineProgress::new("Analyzing Files", files_to_check.len())?;
    let progress_handle = progress.clone_handle();

    let rt =
        tokio::runtime::Runtime::new().map_err(|e| format!("failed to start runtime: {}", e))?;
    let render_handle = rt.block_on(async { progress.start_render_loop(progress_handle.clone()) });

    let mut image_count = 0;
    let mut audio_count = 0;
    let mut audio_duration_secs = 0.0;
    let mut audio_duration_failed = 0;
    let mut audio_error_samples: Vec<String> = Vec::new();
    let mut video_count = 0;
    let mut video_duration_secs = 0.0;
    let mut video_duration_failed = 0;
    let mut video_error_samples: Vec<String> = Vec::new();

    for (i, file_path) in files_to_check.iter().enumerate() {
        let file_name = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let _ = progress_handle.start_task(i, file_name.clone(), 0);
        let task_start = Instant::now();
        if is_metadata_file(file_path) {
            continue;
        }

        if is_mxf_file(file_path) {
            match has_video_streams(file_path) {
                Ok(true) => {
                    video_count += 1;
                    match get_media_duration(file_path) {
                        Ok(duration) => video_duration_secs += duration,
                        Err(e) => {
                            video_duration_failed += 1;
                            if video_error_samples.len() < 3 {
                                video_error_samples.push(format!(
                                    "{}: {}",
                                    file_path
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or("<unknown>"),
                                    e
                                ));
                            }
                            if !e.contains("file format not supported or corrupted") {
                                let _ = progress_handle.add_warning(format!(
                                    "Failed to get duration for {}: {}",
                                    file_name, e
                                ));
                            }
                        }
                    }
                }
                Ok(false) => {
                    audio_count += 1;
                    match get_media_duration(file_path) {
                        Ok(duration) => audio_duration_secs += duration,
                        Err(e) => {
                            audio_duration_failed += 1;
                            if audio_error_samples.len() < 3 {
                                audio_error_samples.push(format!(
                                    "{}: {}",
                                    file_path
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or("<unknown>"),
                                    e
                                ));
                            }
                            if !e.contains("file format not supported or corrupted") {
                                let _ = progress_handle.add_warning(format!(
                                    "Failed to get duration for {}: {}",
                                    file_name, e
                                ));
                            }
                        }
                    }
                }
                Err(_) => {
                    audio_count += 1;
                    match get_media_duration(file_path) {
                        Ok(duration) => audio_duration_secs += duration,
                        Err(e) => {
                            audio_duration_failed += 1;
                            if audio_error_samples.len() < 3 {
                                audio_error_samples.push(format!(
                                    "{}: {}",
                                    file_path
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or("<unknown>"),
                                    e
                                ));
                            }
                            if !e.contains("file format not supported or corrupted") {
                                let _ = progress_handle.add_warning(format!(
                                    "Failed to get duration for {}: {}",
                                    file_name, e
                                ));
                            }
                        }
                    }
                }
            }
        } else if is_image_file(file_path) {
            image_count += 1;
        } else if is_audio_file(file_path) {
            audio_count += 1;
            match get_media_duration(file_path) {
                Ok(duration) => audio_duration_secs += duration,
                Err(e) => {
                    audio_duration_failed += 1;
                    if !e.contains("file format not supported or corrupted") {
                        let _ = progress_handle.add_warning(format!(
                            "Failed to get duration for {}: {}",
                            file_name, e
                        ));
                    }
                }
            }
        } else if has_video_ext(file_path) {
            video_count += 1;
            match get_media_duration(file_path) {
                Ok(duration) => video_duration_secs += duration,
                Err(e) => {
                    video_duration_failed += 1;
                    if !e.contains("file format not supported or corrupted") {
                        let _ = progress_handle.add_warning(format!(
                            "Failed to get duration for {}: {}",
                            file_name, e
                        ));
                    }
                }
            }
        }

        let elapsed = task_start.elapsed();
        let _ = progress_handle.finish_task(i, true);
    }

    let _ = progress_handle.add_success("Analysis complete");
    rt.block_on(async {
        InlineProgress::stop_render_loop(render_handle).await;
    });
    progress.finish()?;

    println!();

    output::success("Analysis complete");
    output::plain("");
    output::info(format!("Images: {}", image_count));

    if audio_duration_failed > 0 {
        output::info(format!(
            "Audio: {} files, {:.2} hours (duration unavailable for {} file(s))",
            audio_count,
            audio_duration_secs / 3600.0,
            audio_duration_failed
        ));
        if !audio_error_samples.is_empty() {
            output::warning("Sample audio duration errors:");
            for err in &audio_error_samples {
                output::item(err);
            }
        }
    } else {
        output::info(format!(
            "Audio: {} files, {:.2} hours",
            audio_count,
            audio_duration_secs / 3600.0
        ));
    }

    if video_duration_failed > 0 {
        output::info(format!(
            "Video: {} files, {:.2} hours (duration unavailable for {} file(s))",
            video_count,
            video_duration_secs / 3600.0,
            video_duration_failed
        ));
        if !video_error_samples.is_empty() {
            output::warning("Sample video duration errors:");
            for err in &video_error_samples {
                output::item(err);
            }
        }
    } else {
        output::info(format!(
            "Video: {} files, {:.2} hours",
            video_count,
            video_duration_secs / 3600.0
        ));
    }

    Ok(())
}
