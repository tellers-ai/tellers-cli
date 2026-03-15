use clap::ValueEnum;
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::{FfmpegEvent, LogLevel};
use std::path::PathBuf;

use crate::media::metadata::get_ffprobe_json;
use crate::media::video_quality::VideoQuality;

/// Returns true if the first video stream is interlaced (field_order is tb, bt, tt, or bb).
fn is_interlaced(path: &PathBuf) -> Result<bool, String> {
    let probe = match get_ffprobe_json(path)? {
        Some(p) => p,
        None => return Ok(false),
    };
    let streams = probe
        .get("streams")
        .and_then(|s| s.as_array())
        .ok_or_else(|| "no streams in ffprobe output".to_string())?;
    let video_stream = streams
        .iter()
        .find(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("video"))
        .ok_or_else(|| "no video stream".to_string())?;
    let field_order = video_stream
        .get("field_order")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_lowercase();
    // Interlaced: tb, bt, tt, bb. Progressive or unknown => not interlaced.
    Ok(matches!(
        field_order.as_str(),
        "tb" | "bt" | "tt" | "bb"
    ))
}

/// Parse ffmpeg time string e.g. "00:01:23.45" into seconds.
fn parse_ffmpeg_time(s: &str) -> Option<f64> {
    let s = s.trim();
    let parts: Vec<&str> = s.splitn(3, ':').collect();
    if parts.len() != 3 {
        return None;
    }
    let hours: f64 = parts[0].trim().parse().ok()?;
    let minutes: f64 = parts[1].trim().parse().ok()?;
    let secs_str = parts[2].trim();
    let seconds: f64 = secs_str.parse().ok()?;
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

#[derive(Copy, Clone, Debug, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Preset {
    VeryFast,
    Fast,
    Medium,
    Slow,
    VerySlow,
}

impl Preset {
    fn as_str(&self) -> &'static str {
        match self {
            Preset::VeryFast => "veryfast",
            Preset::Fast => "fast",
            Preset::Medium => "medium",
            Preset::Slow => "slow",
            Preset::VerySlow => "veryslow",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RenditionDefinition {
    pub quality: Option<VideoQuality>,
    pub preset: Option<Preset>,
    pub crf: Option<u8>,
    pub audio_bitrate: Option<u32>,
}

impl RenditionDefinition {
    pub fn to_name(&self) -> String {
        let mut params = <Vec<String>>::new();
        if let Some(quality) = self.quality {
            params.push(quality.as_label().to_string());
        }
        if let Some(preset) = self.preset {
            params.push(preset.as_str().to_string());
        }
        if let Some(crf) = self.crf {
            params.push(crf.to_string());
        }
        if let Some(audio_bitrate) = self.audio_bitrate {
            params.push(audio_bitrate.to_string());
        }
        params.join("_")
    }
}

fn get_temp_rendition_dir() -> Result<PathBuf, String> {
    let temp_base = std::env::temp_dir().join("tellers-cli").join("renditions");
    if !temp_base.exists() {
        std::fs::create_dir_all(&temp_base).map_err(|e| {
            format!(
                "failed to create temp rendition directory {}: {}",
                temp_base.display(),
                e
            )
        })?;
    }
    Ok(temp_base)
}

fn compute_rendition_output_path(
    input: &PathBuf,
    definition: &RenditionDefinition,
    out_base: &PathBuf,
) -> PathBuf {
    let rendition_name = definition.to_name();
    out_base.join(format!(
        "{}_{}.mp4",
        input.file_stem().unwrap().to_string_lossy(),
        rendition_name
    ))
}

pub fn create_rendition(
    input: &PathBuf,
    definition: RenditionDefinition,
    progress_cb: Option<&mut dyn FnMut(f64)>,
    info_cb: Option<&dyn Fn(&str)>,
) -> Result<PathBuf, String> {
    let temp_base = get_temp_rendition_dir()?;
    let output = compute_rendition_output_path(input, &definition, &temp_base);

    if let (Ok(in_md), Ok(out_md)) = (std::fs::metadata(input), std::fs::metadata(&output)) {
        if let (Ok(in_time), Ok(out_time)) = (in_md.modified(), out_md.modified()) {
            if out_time >= in_time {
                let msg = format!(
                    "Reusing existing rendition for {} at {} ({})",
                    input.display(),
                    output.display(),
                    definition.to_name()
                );
                if let Some(f) = info_cb {
                    f(&msg);
                } else if progress_cb.is_none() {
                    crate::output::info(msg);
                }
                return Ok(output);
            }
        }
    }

    let interlaced = is_interlaced(input).unwrap_or(false);
    if interlaced {
        if let Some(f) = info_cb {
            f("Input is interlaced; deinterlacing to progressive");
        }
    }

    let mut cmd = FfmpegCommand::new();
    cmd.overwrite()
        .input(input.to_string_lossy())
        .codec_video("libx264")
        .args(["-pix_fmt", "yuv420p"]);

    if let Some(q) = definition.quality {
        let height = q.height();
        let vf = if interlaced {
            format!("bwdif=0:-1:0,scale=-2:{}", height)
        } else {
            format!("scale=-2:{}", height)
        };
        cmd.args(["-vf", &vf]);
    } else if interlaced {
        cmd.args(["-vf", "bwdif=0:-1:0"]);
    }
    if let Some(p) = definition.preset {
        cmd.preset(p.as_str());
    }
    if let Some(crf) = definition.crf {
        cmd.crf(crf as u32);
    }

    cmd.args(["-movflags", "+faststart"]).codec_audio("aac");
    if let Some(abr_kbps) = definition.audio_bitrate {
        cmd.args(["-b:a", &format!("{}k", abr_kbps)]);
    }

    if std::env::var("TELLERS_DEBUG_FFMPEG").ok().as_deref() == Some("1") {
        cmd.print_command();
    }

    cmd.output(output.to_string_lossy());

    if let Some(cb) = progress_cb {
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to start ffmpeg: {}", e))?;
        let mut iter = child
            .iter()
            .map_err(|e| format!("failed to create ffmpeg event iterator: {}", e))?;
        let mut total_duration: Option<f64> = None;
        let mut stderr_lines: Vec<String> = Vec::new();
        while let Some(event) = iter.next() {
            match &event {
                FfmpegEvent::ParsedDuration(d) => {
                    total_duration = Some(d.duration);
                }
                FfmpegEvent::Progress(p) => {
                    if let (Some(total), Some(current_secs)) =
                        (total_duration, parse_ffmpeg_time(&p.time))
                    {
                        if total > 0.0 {
                            let pct = (100.0 * current_secs / total).min(100.0);
                            cb(pct);
                        }
                    }
                }
                FfmpegEvent::Error(e) => {
                    stderr_lines.push(e.clone());
                }
                FfmpegEvent::Log(LogLevel::Error, msg) | FfmpegEvent::Log(LogLevel::Fatal, msg) => {
                    stderr_lines.push(msg.clone());
                }
                _ => {}
            }
        }
        let status = child
            .wait()
            .map_err(|e| format!("failed to wait for ffmpeg: {}", e))?;
        if !status.success() {
            let log_suffix = if stderr_lines.is_empty() {
                String::new()
            } else {
                format!("\nffmpeg log:\n{}", stderr_lines.join("\n"))
            };
            return Err(format!(
                "ffmpeg failed creating rendition: {} -> {}{}",
                input.display(),
                output.display(),
                log_suffix
            ));
        }
    } else {
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to start ffmpeg: {}", e))?;
        let status = child
            .wait()
            .map_err(|e| format!("failed to wait for ffmpeg: {}", e))?;
        if !status.success() {
            return Err(format!(
                "ffmpeg failed creating rendition: {} -> {}",
                input.display(),
                output.display()
            ));
        }
    }

    Ok(output)
}

fn compute_audio_output_path(input: &PathBuf, out_base: &PathBuf) -> PathBuf {
    out_base.join(format!(
        "{}.mp3",
        input.file_stem().unwrap().to_string_lossy()
    ))
}

fn compute_normalized_audio_output_path(input: &PathBuf, out_base: &PathBuf) -> PathBuf {
    out_base.join(format!(
        "{}_norm.mp3",
        input.file_stem().unwrap().to_string_lossy()
    ))
}

pub fn normalize_audio_to_mp3(
    input: &PathBuf,
    audio_bitrate: Option<u32>,
    progress_cb: Option<&mut dyn FnMut(f64)>,
    info_cb: Option<&dyn Fn(&str)>,
) -> Result<PathBuf, String> {
    let temp_base = get_temp_rendition_dir()?;
    let output = compute_normalized_audio_output_path(input, &temp_base);

    if let (Ok(in_md), Ok(out_md)) = (std::fs::metadata(input), std::fs::metadata(&output)) {
        if let (Ok(in_time), Ok(out_time)) = (in_md.modified(), out_md.modified()) {
            if out_time >= in_time {
                let msg = format!(
                    "Reusing existing normalized MP3 for {} at {}",
                    input.display(),
                    output.display()
                );
                if let Some(f) = info_cb {
                    f(&msg);
                } else if progress_cb.is_none() {
                    crate::output::info(msg);
                }
                return Ok(output);
            }
        }
    }

    let bitrate_k = audio_bitrate.unwrap_or(192);
    let loudnorm = "loudnorm=I=-14:LRA=11:TP=-1.5";

    let mut cmd = FfmpegCommand::new();
    cmd.overwrite()
        .input(input.to_string_lossy())
        .args(["-af", loudnorm])
        .codec_audio("libmp3lame")
        .args(["-b:a", &format!("{}k", bitrate_k)]);

    if std::env::var("TELLERS_DEBUG_FFMPEG").ok().as_deref() == Some("1") {
        cmd.print_command();
    }

    cmd.output(output.to_string_lossy());

    run_ffmpeg_with_progress(
        cmd,
        input,
        &output,
        "normalizing audio to MP3",
        progress_cb,
    )
}

fn run_ffmpeg_with_progress(
    mut cmd: FfmpegCommand,
    input: &PathBuf,
    output: &PathBuf,
    operation: &str,
    progress_cb: Option<&mut dyn FnMut(f64)>,
) -> Result<PathBuf, String> {
    if let Some(cb) = progress_cb {
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to start ffmpeg: {}", e))?;
        let mut iter = child
            .iter()
            .map_err(|e| format!("failed to create ffmpeg event iterator: {}", e))?;
        let mut total_duration: Option<f64> = None;
        let mut stderr_lines: Vec<String> = Vec::new();
        while let Some(event) = iter.next() {
            match &event {
                FfmpegEvent::ParsedDuration(d) => {
                    total_duration = Some(d.duration);
                }
                FfmpegEvent::Progress(p) => {
                    if let (Some(total), Some(current_secs)) =
                        (total_duration, parse_ffmpeg_time(&p.time))
                    {
                        if total > 0.0 {
                            let pct = (100.0 * current_secs / total).min(100.0);
                            cb(pct);
                        }
                    }
                }
                FfmpegEvent::Error(e) => {
                    stderr_lines.push(e.clone());
                }
                FfmpegEvent::Log(LogLevel::Error, msg) | FfmpegEvent::Log(LogLevel::Fatal, msg) => {
                    stderr_lines.push(msg.clone());
                }
                _ => {}
            }
        }
        let status = child
            .wait()
            .map_err(|e| format!("failed to wait for ffmpeg: {}", e))?;
        if !status.success() {
            let log_suffix = if stderr_lines.is_empty() {
                String::new()
            } else {
                format!("\nffmpeg log:\n{}", stderr_lines.join("\n"))
            };
            return Err(format!(
                "ffmpeg failed {}: {} -> {}{}",
                operation,
                input.display(),
                output.display(),
                log_suffix
            ));
        }
        Ok(output.clone())
    } else {
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to start ffmpeg: {}", e))?;
        let status = child
            .wait()
            .map_err(|e| format!("failed to wait for ffmpeg: {}", e))?;
        if !status.success() {
            return Err(format!(
                "ffmpeg failed {}: {} -> {}",
                operation,
                input.display(),
                output.display()
            ));
        }
        Ok(output.clone())
    }
}

pub fn convert_to_mp3(
    input: &PathBuf,
    audio_bitrate: Option<u32>,
    progress_cb: Option<&mut dyn FnMut(f64)>,
    info_cb: Option<&dyn Fn(&str)>,
) -> Result<PathBuf, String> {
    let temp_base = get_temp_rendition_dir()?;
    let output = compute_audio_output_path(input, &temp_base);

    if let (Ok(in_md), Ok(out_md)) = (std::fs::metadata(input), std::fs::metadata(&output)) {
        if let (Ok(in_time), Ok(out_time)) = (in_md.modified(), out_md.modified()) {
            if out_time >= in_time {
                let msg = format!(
                    "Reusing existing MP3 conversion for {} at {}",
                    input.display(),
                    output.display()
                );
                if let Some(f) = info_cb {
                    f(&msg);
                } else if progress_cb.is_none() {
                    crate::output::info(msg);
                }
                return Ok(output);
            }
        }
    }

    let mut cmd = FfmpegCommand::new();
    cmd.overwrite()
        .input(input.to_string_lossy())
        .codec_audio("libmp3lame");

    if let Some(abr_kbps) = audio_bitrate {
        cmd.args(["-b:a", &format!("{}k", abr_kbps)]);
    } else {
        cmd.args(["-b:a", "192k"]);
    }

    if std::env::var("TELLERS_DEBUG_FFMPEG").ok().as_deref() == Some("1") {
        cmd.print_command();
    }

    cmd.output(output.to_string_lossy());

    run_ffmpeg_with_progress(
        cmd,
        input,
        &output,
        "converting to MP3",
        progress_cb,
    )
}

pub fn is_mxf_file(path: &PathBuf) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("").eq_ignore_ascii_case("mxf")
}

pub fn has_video_streams(path: &PathBuf) -> Result<bool, String> {
    use std::process::Command;

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_type",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            &path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("failed to run ffprobe: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let output_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(output_str == "video")
}


