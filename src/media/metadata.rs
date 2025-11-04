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
    // Try to extract UMID from format tags (general formats)
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

    if output.status.success() {
        let output_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output_str.is_empty() {
            return Ok(Some(output_str));
        }
    }

    // For MXF files, try to extract material_package_umid from format metadata
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format_tags=material_package_umid",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            &path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("failed to run ffprobe for material_package_umid extraction: {}", e))?;

    if output.status.success() {
        let output_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output_str.is_empty() {
            // Remove the "0x" prefix if present
            let umid = output_str.strip_prefix("0x").unwrap_or(&output_str).to_string();
            return Ok(Some(umid));
        }
    }

    Ok(None)
}
