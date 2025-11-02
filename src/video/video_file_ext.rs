use std::path::Path;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum VideoFileExt {
    Mp4,
    Mov,
    Mxf,
    Mkv,
    Webm,
    Avi,
    M4v,
    Mpg,
    Mpeg,
    Wmv,
}

impl VideoFileExt {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "mp4" => Some(VideoFileExt::Mp4),
            "mov" => Some(VideoFileExt::Mov),
            "mxf" => Some(VideoFileExt::Mxf),
            "mkv" => Some(VideoFileExt::Mkv),
            "webm" => Some(VideoFileExt::Webm),
            "avi" => Some(VideoFileExt::Avi),
            "m4v" => Some(VideoFileExt::M4v),
            "mpg" => Some(VideoFileExt::Mpg),
            "mpeg" => Some(VideoFileExt::Mpeg),
            "wmv" => Some(VideoFileExt::Wmv),
            _ => None,
        }
    }
}

pub fn has_video_ext(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    VideoFileExt::from_str(&ext).is_some()
}
