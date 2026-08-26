use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

const AUTH_BASE: &str = "https://auth.tellers.ai";
// The API accepts the OAuth access token for the same audience used by the
// hosted MCP resource; the backend deliberately keeps this audience separate
// from its HTTP origin.
const RESOURCE: &str = "https://mcp.tellers.ai";

#[derive(Debug, Serialize, Deserialize)]
struct StoredSession {
    client_id: String,
    access_token: String,
    refresh_token: String,
    expires_at: u64,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
}

fn session_path() -> Result<PathBuf, String> {
    dirs::config_dir()
        .or_else(dirs::data_local_dir)
        .map(|dir| dir.join("tellers").join("auth.json"))
        .ok_or_else(|| "Could not determine a configuration directory".to_string())
}

fn load_session() -> Result<Option<StoredSession>, String> {
    let path = session_path()?;
    match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents)
            .map(Some)
            .map_err(|e| format!("Invalid saved Tellers login: {e}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Could not read saved Tellers login: {error}")),
    }
}

fn save_session(session: &StoredSession) -> Result<(), String> {
    let path = session_path()?;
    let parent = path.parent().expect("auth path has a parent");
    fs::create_dir_all(parent).map_err(|e| format!("Could not create config directory: {e}"))?;
    let contents = serde_json::to_vec_pretty(session).map_err(|e| e.to_string())?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&path)
        .map_err(|e| format!("Could not save Tellers login: {e}"))?;
    file.write_all(&contents)
        .map_err(|e| format!("Could not save Tellers login: {e}"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn saved_access_token() -> Option<String> {
    load_session()
        .ok()
        .flatten()
        .map(|session| session.access_token)
}

pub async fn refresh_if_needed() -> Result<(), String> {
    let Some(session) = load_session()? else {
        return Ok(());
    };
    // Keep a small safety margin so a request cannot start with a token that is
    // about to expire. Refresh token rotation is persisted atomically enough for
    // this single-user CLI: write only after the server returns both new tokens.
    if session.expires_at > now() + 60 {
        return Ok(());
    }
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{AUTH_BASE}/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", session.refresh_token.as_str()),
            ("client_id", session.client_id.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("Could not refresh Tellers login: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Tellers login expired or was revoked (HTTP {}) — run `tellers login` again",
            response.status()
        ));
    }
    let token: TokenResponse = response
        .json()
        .await
        .map_err(|e| format!("Invalid token response from Tellers: {e}"))?;
    save_session(&StoredSession {
        client_id: session.client_id,
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at: now() + token.expires_in,
    })
}

fn pkce() -> (String, String) {
    let verifier = uuid::Uuid::new_v4().to_string().replace('-', "");
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    (verifier, challenge)
}

fn callback(listener: &TcpListener, expected_state: &str) -> Result<String, String> {
    listener.set_nonblocking(false).map_err(|e| e.to_string())?;
    let (mut stream, _) = listener
        .accept()
        .map_err(|e| format!("Login callback failed: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(300)))
        .map_err(|e| e.to_string())?;
    let mut request = [0u8; 8192];
    let length = stream.read(&mut request).map_err(|e| e.to_string())?;
    let request = String::from_utf8_lossy(&request[..length]);
    let target = request
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("GET "))
        .and_then(|line| line.split_whitespace().next())
        .ok_or_else(|| "Invalid login callback".to_string())?;
    let url = Url::parse(&format!("http://localhost{target}"))
        .map_err(|e| format!("Invalid login callback URL: {e}"))?;
    let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    let html = "<h1>Tellers login complete</h1><p>You can close this window.</p>";
    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", html.len(), html);
    let _ = stream.write_all(response.as_bytes());
    if params.get("state").map(String::as_str) != Some(expected_state) {
        return Err("Login callback state did not match".to_string());
    }
    params.get("code").cloned().ok_or_else(|| {
        params
            .get("error_description")
            .cloned()
            .unwrap_or_else(|| "Tellers login was denied".to_string())
    })
}

pub async fn login() -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Could not open login callback: {e}"))?;
    let redirect_uri = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr().unwrap().port()
    );
    let client = reqwest::Client::new();
    let registration = client
        .post(format!("{AUTH_BASE}/oauth/register"))
        .json(&serde_json::json!({
            "redirect_uris": [redirect_uri],
            "client_name": "Tellers CLI",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none"
        }))
        .send()
        .await
        .map_err(|e| format!("Could not register Tellers CLI: {e}"))?;
    if !registration.status().is_success() {
        return Err(format!(
            "Could not register Tellers CLI (HTTP {})",
            registration.status()
        ));
    }
    let client_id: String = registration
        .json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())?
        .get("client_id")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| "Tellers registration returned no client_id".to_string())?;
    let (verifier, challenge) = pkce();
    let state = uuid::Uuid::new_v4().to_string();
    let mut authorize = Url::parse(&format!("{AUTH_BASE}/oauth/authorize")).unwrap();
    authorize
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("resource", RESOURCE);
    let authorize = authorize.to_string();
    println!("Opening Tellers login in your browser...");
    let opened = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg(&authorize)
            .status()
            .is_ok()
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", &authorize])
            .status()
            .is_ok()
    } else {
        std::process::Command::new("xdg-open")
            .arg(&authorize)
            .status()
            .is_ok()
    };
    if !opened {
        println!("Open this URL manually:\n{authorize}");
    }
    let code = tokio::task::spawn_blocking(move || callback(&listener, &state))
        .await
        .map_err(|e| format!("Login callback failed: {e}"))??;
    let token = client
        .post(format!("{AUTH_BASE}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("Could not exchange login code: {e}"))?;
    if !token.status().is_success() {
        return Err(format!(
            "Could not complete Tellers login (HTTP {})",
            token.status()
        ));
    }
    let token: TokenResponse = token.json().await.map_err(|e| e.to_string())?;
    save_session(&StoredSession {
        client_id,
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at: now() + token.expires_in,
    })?;
    println!("Logged in to Tellers.");
    Ok(())
}

pub fn logout() -> Result<(), String> {
    match session_path().and_then(|path| {
        fs::remove_file(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "not-found".to_string()
            } else {
                error.to_string()
            }
        })
    }) {
        Ok(()) => println!("Logged out of Tellers."),
        Err(error) if error == "not-found" => println!("No saved Tellers login."),
        Err(error) => return Err(format!("Could not remove saved Tellers login: {error}")),
    }
    Ok(())
}
