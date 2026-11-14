//! Checks GitHub for a newer `tellers` release and tells the user how to upgrade.
//!
//! The automatic check runs at most once per day (the result is cached in the
//! config directory), uses a short timeout, prints to stderr only, and never
//! fails the command. It is skipped when stderr is not a terminal, in CI, or
//! when `TELLERS_NO_UPDATE_CHECK` is set.

use std::fs;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/tellers-ai/tellers-cli/releases/latest";
const RELEASES_PAGE: &str = "https://github.com/tellers-ai/tellers-cli/releases/latest";
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const AUTO_CHECK_TIMEOUT: Duration = Duration::from_secs(2);
const MANUAL_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Serialize, Deserialize)]
struct Cache {
    checked_at: u64,
    latest: String,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
}

/// Automatic, best-effort check run before every command.
pub fn auto_check() {
    if std::env::var_os("TELLERS_NO_UPDATE_CHECK").is_some()
        || std::env::var_os("CI").is_some()
        || !std::io::stderr().is_terminal()
    {
        return;
    }

    let now = now_secs();
    let latest = match read_cache() {
        Some(cache) if now.saturating_sub(cache.checked_at) < CHECK_INTERVAL.as_secs() => {
            cache.latest
        }
        _ => match fetch_latest(AUTO_CHECK_TIMEOUT) {
            Ok(latest) => {
                write_cache(&Cache {
                    checked_at: now,
                    latest: latest.clone(),
                });
                latest
            }
            Err(error) => {
                log::debug!("update check failed: {error}");
                // Don't retry on every command while offline.
                write_cache(&Cache {
                    checked_at: now,
                    latest: CURRENT_VERSION.to_string(),
                });
                return;
            }
        },
    };

    if is_newer(&latest, CURRENT_VERSION) {
        eprintln!("{}", update_notice(&latest));
    }
}

/// `tellers update`: always queries GitHub and reports the result.
pub fn run() -> Result<(), String> {
    let latest = fetch_latest(MANUAL_CHECK_TIMEOUT)?;
    write_cache(&Cache {
        checked_at: now_secs(),
        latest: latest.clone(),
    });
    if is_newer(&latest, CURRENT_VERSION) {
        println!("{}", update_notice(&latest));
    } else {
        println!("tellers {CURRENT_VERSION} is up to date.");
    }
    Ok(())
}

fn update_notice(latest: &str) -> String {
    format!(
        "A new version of tellers is available: {CURRENT_VERSION} → {latest}\nUpgrade with: {}",
        upgrade_hint()
    )
}

fn upgrade_hint() -> String {
    let installed_with_brew = std::env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok())
        .map(|path| {
            let path = path.to_string_lossy().to_lowercase();
            path.contains("/cellar/") || path.contains("/homebrew/")
        })
        .unwrap_or(false);
    if installed_with_brew {
        "brew upgrade tellers".to_string()
    } else {
        format!("download it from {RELEASES_PAGE}")
    }
}

fn fetch_latest(timeout: Duration) -> Result<String, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Could not start runtime: {e}"))?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(format!("tellers-cli/{CURRENT_VERSION}"))
            .build()
            .map_err(|e| format!("Could not build HTTP client: {e}"))?;
        let response = client
            .get(LATEST_RELEASE_URL)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| format!("Could not reach GitHub: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("GitHub returned {}", response.status()));
        }
        let release: Release = response
            .json()
            .await
            .map_err(|e| format!("Invalid release response: {e}"))?;
        Ok(release.tag_name.trim_start_matches('v').to_string())
    })
}

/// True when `candidate` is a strictly higher `major.minor.patch` than `current`.
fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let core = version
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let major = parts.next()??;
    let minor = parts.next().unwrap_or(Some(0))?;
    let patch = parts.next().unwrap_or(Some(0))?;
    Some((major, minor, patch))
}

fn cache_path() -> Option<PathBuf> {
    dirs::config_dir()
        .or_else(dirs::data_local_dir)
        .map(|dir| dir.join("tellers").join("update_check.json"))
}

fn read_cache() -> Option<Cache> {
    let contents = fs::read_to_string(cache_path()?).ok()?;
    serde_json::from_str(&contents).ok()
}

fn write_cache(cache: &Cache) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(contents) = serde_json::to_string(cache) {
        let _ = fs::write(path, contents);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.0.10", "0.0.9"));
        assert!(is_newer("v0.1.0", "0.0.5"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.0.5", "0.0.5"));
        assert!(!is_newer("0.0.4", "0.0.5"));
    }

    #[test]
    fn ignores_unparseable_versions() {
        assert!(!is_newer("latest", "0.0.5"));
        assert!(!is_newer("", "0.0.5"));
    }

    #[test]
    fn strips_prerelease_suffix() {
        assert_eq!(parse_version("v1.2.3-beta.1"), Some((1, 2, 3)));
    }
}
