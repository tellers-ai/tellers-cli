use std::path::PathBuf;
use std::process::Command;

pub struct MediaMetadata {
    pub umid: Option<String>,
}

pub fn extract_media_metadata(path: &PathBuf) -> Result<MediaMetadata, String> {
    let umid = extract_umid(path)?;

    Ok(MediaMetadata { umid })
}

fn extract_umid(path: &PathBuf) -> Result<Option<String>, String> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format_tags=umid:stream_tags=umid",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            &path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("failed to run ffprobe for umid extraction: {}", e))?;

    if !output.status.success() {
        return Ok(None);
    }

    let output_str = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if output_str.is_empty() {
        Ok(None)
    } else {
        Ok(Some(output_str))
    }
}
