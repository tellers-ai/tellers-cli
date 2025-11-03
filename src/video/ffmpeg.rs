use ffmpeg_sidecar::download;
use std::path::PathBuf;

pub fn ensure_ffmpeg_available() -> Result<(), String> {
    download::auto_download().map_err(|e| format!("failed to prepare ffmpeg binary: {}", e))
}

pub fn get_media_duration(path: &PathBuf) -> Result<f64, String> {
    use std::process::Command;

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            &path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("failed to run ffprobe: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Invalid data found when processing input") {
            return try_alternative_duration_method(path);
        }
        return Err(format!("ffprobe failed: {}", stderr));
    }

    let duration_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if duration_str.is_empty() {
        return try_alternative_duration_method(path);
    }
    
    match duration_str.parse::<f64>() {
        Ok(d) if d > 0.0 => Ok(d),
        _ => try_alternative_duration_method(path),
    }
}

fn try_alternative_duration_method(path: &PathBuf) -> Result<f64, String> {
    use std::process::Command;

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams", "a:0",
            "-show_entries", "stream=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            &path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("failed to run ffprobe (stream method): {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Invalid data found when processing input") {
            return Err("file format not supported or corrupted".to_string());
        }
        return Err(format!("ffprobe failed: {}", stderr));
    }

    let duration_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if duration_str.is_empty() {
        return Err("no duration found in file".to_string());
    }
    duration_str
        .parse::<f64>()
        .map_err(|e| format!("failed to parse duration '{}': {}", duration_str, e))
}
