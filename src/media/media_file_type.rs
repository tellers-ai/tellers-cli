use std::path::PathBuf;

pub fn is_image_file(path: &PathBuf) -> bool {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    mime.type_() == "image"
}

pub fn is_audio_file(path: &PathBuf) -> bool {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    if mime.type_() == "audio" {
        return true;
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "wma"
    )
}

pub fn is_metadata_file(path: &PathBuf) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|name| name.starts_with("._"))
        .unwrap_or(false)
}
