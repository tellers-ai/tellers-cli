use ffmpeg_sidecar::download;

pub fn ensure_ffmpeg_available() -> Result<(), String> {
    download::auto_download().map_err(|e| format!("failed to prepare ffmpeg binary: {}", e))
}
