#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum VideoQuality {
    P144,
    P240,
    P360,
    P480,
    P720,
    P1080,
}

impl VideoQuality {
    pub fn height(self) -> u32 {
        match self {
            VideoQuality::P144 => 144,
            VideoQuality::P240 => 240,
            VideoQuality::P360 => 360,
            VideoQuality::P480 => 480,
            VideoQuality::P720 => 720,
            VideoQuality::P1080 => 1080,
        }
    }

    pub fn as_label(self) -> &'static str {
        match self {
            VideoQuality::P144 => "144p",
            VideoQuality::P240 => "240p",
            VideoQuality::P360 => "360p",
            VideoQuality::P480 => "480p",
            VideoQuality::P720 => "720p",
            VideoQuality::P1080 => "1080p",
        }
    }
}

impl std::fmt::Display for VideoQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_label())
    }
}

pub fn parse_quality(s: &str) -> Result<VideoQuality, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "144p" | "144" => Ok(VideoQuality::P144),
        "240p" | "240" => Ok(VideoQuality::P240),
        "360p" | "360" => Ok(VideoQuality::P360),
        "480p" | "480" => Ok(VideoQuality::P480),
        "720p" | "720" => Ok(VideoQuality::P720),
        "1080p" | "1080" => Ok(VideoQuality::P1080),
        other => Err(format!(
            "invalid quality '{}'; expected one of: 144p, 240p, 360p, 480p, 720p, 1080p",
            other
        )),
    }
}


