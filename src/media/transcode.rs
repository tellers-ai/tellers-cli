use clap::ValueEnum;
use ffmpeg_sidecar::command::FfmpegCommand;
use std::path::PathBuf;

use crate::media::video_quality::VideoQuality;

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
) -> Result<PathBuf, String> {
    let temp_base = get_temp_rendition_dir()?;
    let output = compute_rendition_output_path(input, &definition, &temp_base);

    if let (Ok(in_md), Ok(out_md)) = (std::fs::metadata(input), std::fs::metadata(&output)) {
        if let (Ok(in_time), Ok(out_time)) = (in_md.modified(), out_md.modified()) {
            if out_time >= in_time {
                println!(
                    "  reusing existing rendition for {} at {} ({})",
                    input.display(),
                    output.display(),
                    definition.to_name()
                );
                return Ok(output);
            }
        }
    }

    let mut cmd = FfmpegCommand::new();
    cmd.overwrite()
        .input(input.to_string_lossy().to_string())
        .codec_video("libx264");

    if let Some(q) = definition.quality {
        let height = q.height();
        cmd.args(["-vf", &format!("scale=-2:{}", height)]);
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

    cmd.output(output.to_string_lossy().to_string());

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

    Ok(output)
}

fn compute_audio_output_path(input: &PathBuf, out_base: &PathBuf) -> PathBuf {
    out_base.join(format!(
        "{}.mp3",
        input.file_stem().unwrap().to_string_lossy()
    ))
}

pub fn convert_to_mp3(input: &PathBuf, audio_bitrate: Option<u32>) -> Result<PathBuf, String> {
    let temp_base = get_temp_rendition_dir()?;
    let output = compute_audio_output_path(input, &temp_base);

    if let (Ok(in_md), Ok(out_md)) = (std::fs::metadata(input), std::fs::metadata(&output)) {
        if let (Ok(in_time), Ok(out_time)) = (in_md.modified(), out_md.modified()) {
            if out_time >= in_time {
                println!(
                    "  reusing existing MP3 conversion for {} at {}",
                    input.display(),
                    output.display()
                );
                return Ok(output);
            }
        }
    }

    let mut cmd = FfmpegCommand::new();
    cmd.overwrite()
        .input(input.to_string_lossy().to_string())
        .codec_audio("libmp3lame");

    if let Some(abr_kbps) = audio_bitrate {
        cmd.args(["-b:a", &format!("{}k", abr_kbps)]);
    } else {
        cmd.args(["-b:a", "192k"]);
    }

    if std::env::var("TELLERS_DEBUG_FFMPEG").ok().as_deref() == Some("1") {
        cmd.print_command();
    }

    cmd.output(output.to_string_lossy().to_string());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start ffmpeg: {}", e))?;
    let status = child
        .wait()
        .map_err(|e| format!("failed to wait for ffmpeg: {}", e))?;
    if !status.success() {
        return Err(format!(
            "ffmpeg failed converting to MP3: {} -> {}",
            input.display(),
            output.display()
        ));
    }

    Ok(output)
}

pub fn is_mxf_file(path: &PathBuf) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        == "mxf"
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


