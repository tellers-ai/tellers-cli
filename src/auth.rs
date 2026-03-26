use base64::Engine;

pub fn get_user_id_from_bearer(bearer: Option<&str>) -> String {
    get_user_id_from_bearer_with_logging(bearer, true)
}

pub fn get_user_id_from_bearer_with_logging(bearer: Option<&str>, with_logging: bool) -> String {
    if bearer.is_none() || bearer.map(|s| s.is_empty()).unwrap_or(true) {
        if with_logging {
            crate::output::warning("Bearer token is None or empty, using default user_id");
        }
        return "__current_user__".to_string();
    }

    let bearer_str = bearer.unwrap();
    let token = if bearer_str.starts_with("Bearer ") {
        &bearer_str[7..]
    } else {
        bearer_str
    };

    match decode_jwt_sub(token) {
        Ok(user_id) => {
            if with_logging {
                crate::output::info(format!("Successfully extracted user_id: {}", user_id));
            }
            user_id
        }
        Err(e) => {
            if with_logging {
                crate::output::warning(format!(
                    "Failed to decode JWT: {}, using default user_id",
                    e
                ));
            }
            "__current_user__".to_string()
        }
    }
}

fn decode_jwt_sub(token: &str) -> Result<String, String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(format!(
            "Invalid JWT format: expected 3 parts, got {}",
            parts.len()
        ));
    }

    let payload = parts[1];

    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| {
            let mut padded = payload.to_string();
            while padded.len() % 4 != 0 {
                padded.push('=');
            }
            base64::engine::general_purpose::URL_SAFE.decode(padded.as_bytes())
        })
        .map_err(|e| format!("Failed to decode JWT payload: {}", e))?;

    let json: serde_json::Value = serde_json::from_slice(&decoded)
        .map_err(|e| format!("Failed to parse JWT payload: {}", e))?;

    json.get("sub")
        .or_else(|| json.get("user_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            format!(
                "No sub or user_id in JWT. Available keys: {:?}",
                json.as_object()
                    .map(|o| o.keys().collect::<Vec<_>>())
                    .unwrap_or_default()
            )
        })
}
