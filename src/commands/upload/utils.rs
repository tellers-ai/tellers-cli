use std::path::PathBuf;

use crate::output;
use crate::uploads_tracking;

pub fn compute_in_app_path(
    file_path: &PathBuf,
    base_dir: &PathBuf,
    in_app_path_prefix: &Option<String>,
) -> String {
    let rel_path = if base_dir.is_dir() {
        file_path
            .strip_prefix(base_dir)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string()
    } else {
        file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    };

    match in_app_path_prefix {
        Some(prefix) if !prefix.is_empty() => {
            if base_dir.is_dir() {
                format!("{}/{}", prefix.trim_end_matches('/'), rel_path)
            } else {
                prefix.clone()
            }
        }
        _ => rel_path,
    }
}

pub fn is_already_uploaded(
    file_path: &PathBuf,
    user_id: &str,
    base_dir: &PathBuf,
    in_app_path_prefix: &Option<String>,
) -> bool {
    let in_app_path = compute_in_app_path(file_path, base_dir, in_app_path_prefix);
    match uploads_tracking::is_file_uploaded(user_id, &in_app_path) {
        Ok(true) => {
            output::info(format!(
                "Skipping {} (already uploaded as {})",
                file_path.display(),
                in_app_path
            ));
            true
        }
        Ok(false) => false,
        Err(e) => {
            output::warning(format!(
                "Failed to check upload history for {}: {}",
                file_path.display(),
                e
            ));
            false
        }
    }
}


