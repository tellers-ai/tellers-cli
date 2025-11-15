use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const UPLOADS_FILE_VERSION: &str = "0.0.1";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UploadedFile {
    pub local_path: String,
    pub in_app_path: String,
    pub asset_id: String,
    pub upload_request_id: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct UserUploads {
    files: Vec<UploadedFile>,
}

#[derive(Serialize, Deserialize, Debug)]
struct UploadsFile {
    version: String,
    users: HashMap<String, UserUploads>,
}

pub fn get_uploads_file_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "Unable to find home directory".to_string())?;
    let tellers_dir = home.join(".tellers");
    Ok(tellers_dir.join("uploads.json"))
}

fn ensure_tellers_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "Unable to find home directory".to_string())?;
    let tellers_dir = home.join(".tellers");

    if !tellers_dir.exists() {
        fs::create_dir_all(&tellers_dir)
            .map_err(|e| format!("Failed to create .tellers directory: {}", e))?;
    }

    Ok(tellers_dir)
}

fn load_uploads_file(file_path: &Path) -> Result<UploadsFile, String> {
    if !file_path.exists() {
        return Ok(UploadsFile {
            version: UPLOADS_FILE_VERSION.to_string(),
            users: HashMap::new(),
        });
    }

    let content =
        fs::read_to_string(file_path).map_err(|e| format!("Failed to read uploads file: {}", e))?;

    serde_json::from_str(&content).map_err(|e| format!("Failed to parse uploads file: {}", e))
}

fn save_uploads_file(file_path: &Path, data: &UploadsFile) -> Result<(), String> {
    let json = serde_json::to_string_pretty(data)
        .map_err(|e| format!("Failed to serialize uploads data: {}", e))?;

    fs::write(file_path, json).map_err(|e| format!("Failed to write uploads file: {}", e))?;

    Ok(())
}

pub fn record_upload(
    user_id: &str,
    local_path: &Path,
    in_app_path: &str,
    asset_id: &str,
    upload_request_id: &str,
) -> Result<(), String> {
    let file_path = get_uploads_file_path()?;
    ensure_tellers_dir()?;

    let mut uploads = load_uploads_file(&file_path)?;

    let user_uploads = uploads
        .users
        .entry(user_id.to_string())
        .or_insert_with(|| UserUploads { files: Vec::new() });

    let local_path_str = local_path.to_string_lossy().to_string();

    user_uploads.files.push(UploadedFile {
        local_path: local_path_str,
        in_app_path: in_app_path.to_string(),
        asset_id: asset_id.to_string(),
        upload_request_id: upload_request_id.to_string(),
    });

    save_uploads_file(&file_path, &uploads)
}

pub fn is_file_uploaded(user_id: &str, in_app_path: &str) -> Result<bool, String> {
    let file_path = get_uploads_file_path()?;
    if !file_path.exists() {
        return Ok(false);
    }

    let uploads = load_uploads_file(&file_path)?;

    if let Some(user_uploads) = uploads.users.get(user_id) {
        for uploaded_file in &user_uploads.files {
            if uploaded_file.in_app_path == in_app_path && !uploaded_file.asset_id.is_empty() {
                return Ok(true);
            }
        }
    }

    Ok(false)
}
