use tellers_api_client::apis::configuration::Configuration;

pub fn get_api_base() -> String {
    std::env::var("TELLERS_API_BASE")
        .unwrap_or_else(|_| "https://api.prod.aws.tellers.ai".to_string())
}

pub fn get_api_key(api_key_arg: Option<String>) -> Result<String, String> {
    api_key_arg
        .or_else(|| std::env::var("TELLERS_API_KEY").ok())
        .or_else(|| crate::commands::auth::saved_access_token().map(|_| String::new()))
        .ok_or_else(|| {
            "TELLERS_API_KEY not set; run `tellers login` or set TELLERS_API_KEY".to_string()
        })
}

pub fn get_bearer_header(auth_bearer_arg: Option<String>) -> Option<String> {
    let bearer_env = auth_bearer_arg
        .or_else(|| std::env::var("TELLERS_AUTH_BEARER").ok())
        .or_else(crate::commands::auth::saved_access_token)
        .filter(|v| !v.is_empty());

    bearer_env.as_deref().map(|b| {
        if b.starts_with("Bearer ") {
            b.to_string()
        } else {
            format!("Bearer {}", b)
        }
    })
}

pub fn create_config() -> Configuration {
    let mut cfg = Configuration::default();
    cfg.base_path = get_api_base();
    cfg
}

pub fn format_api_error<E: std::fmt::Debug>(e: &tellers_api_client::apis::Error<E>) -> String {
    let mut message = format!("{}", e);
    match e {
        tellers_api_client::apis::Error::Reqwest(req_err) => {
            if let Some(status) = req_err.status() {
                message.push_str(&format!("; http_status: {}", status));
            }
        }
        tellers_api_client::apis::Error::ResponseError(resp) => {
            message.push_str(&format!("; http_status: {}", resp.status));
            if !resp.content.is_empty() {
                message.push_str(&format!("; response: {}", resp.content));
            }
        }
        _ => {}
    }
    message
}
