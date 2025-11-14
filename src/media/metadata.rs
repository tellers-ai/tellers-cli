use regex::Regex;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

pub struct MediaMetadata {
    pub material_package_umid: Option<String>,
    pub file_package_umids: Vec<String>,
}

pub fn extract_media_metadata(path: &PathBuf) -> Result<MediaMetadata, String> {
    let umids = extract_mxf_umids(path)?;

    Ok(MediaMetadata {
        material_package_umid: umids.material_package_umid,
        file_package_umids: umids.file_package_umids,
    })
}

#[derive(Debug, Default)]
pub struct MxfUmids {
    pub material_package_umid: Option<String>,
    pub file_package_umids: Vec<String>,
}

fn run_ffprobe_json(path: &PathBuf) -> Result<Option<Value>, String> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-of",
            "json",
            &path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("failed to run ffprobe: {}", e))?;

    if !output.status.success() {
        return Ok(None);
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&json_str)
        .map_err(|e| format!("failed to parse ffprobe JSON output: {}", e))
        .map(Some)
}

/// Normalise UMID strings by stripping prefixes and non-hex characters.
///
/// This function sanitizes UMID (Unique Material Identifier) strings by:
/// - Trimming whitespace
/// - Removing the `0x` or `0X` prefix (case-insensitive)
/// - Removing all non-hexadecimal characters
/// - Converting to uppercase
///
/// # Arguments
///
/// * `value` - An optional string slice containing the UMID value to sanitize
///
/// # Returns
///
/// Returns `Some(String)` with the sanitized UMID if the input is valid and non-empty,
/// or `None` if the input is `None`, empty, or contains no valid hexadecimal characters.
///
/// ```
fn sanitize_umid(value: Option<&str>) -> Option<String> {
    let value = value?;
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    let value = value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")).unwrap_or(value);

    static HEX_CLEAN_RE: OnceLock<Regex> = OnceLock::new();
    let hex_clean_re = HEX_CLEAN_RE.get_or_init(|| Regex::new(r"[^0-9A-Fa-f]").unwrap());
    let cleaned = hex_clean_re.replace_all(value, "").to_string().to_uppercase();

    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn extract_mxf_umids(path: &PathBuf) -> Result<MxfUmids, String> {
    let payload = match run_ffprobe_json(path)? {
        Some(p) => p,
        None => {
            return Ok(MxfUmids {
                material_package_umid: None,
                file_package_umids: Vec::new(),
            });
        }
    };

    let mut material = None;
    let mut file_package_ids: Vec<String> = Vec::new();

    if let Some(format) = payload.get("format") {
        if let Some(tags) = format.get("tags") {
            if let Some(tags_obj) = tags.as_object() {
                if let Some(umid_value) = tags_obj.get("material_package_umid") {
                    material = sanitize_umid(umid_value.as_str());
                }
            }
        }
    }

    if let Some(streams) = payload.get("streams") {
        if let Some(streams_array) = streams.as_array() {
            for stream in streams_array {
                if let Some(tags) = stream.get("tags") {
                    if let Some(tags_obj) = tags.as_object() {
                        if let Some(umid_value) = tags_obj.get("file_package_umid") {
                            if let Some(file_umid) = sanitize_umid(umid_value.as_str()) {
                                if !file_package_ids.contains(&file_umid) {
                                    file_package_ids.push(file_umid);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(MxfUmids {
        material_package_umid: material,
        file_package_umids: file_package_ids,
    })
}
