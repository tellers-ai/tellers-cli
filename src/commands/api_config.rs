use tellers_api_client::apis::configuration::Configuration;

pub fn get_api_base() -> String {
    std::env::var("TELLERS_API_BASE")
        .unwrap_or_else(|_| "https://api.prod.aws.tellers.ai".to_string())
}

pub fn get_api_key(api_key_arg: Option<String>) -> Result<String, String> {
    api_key_arg
        .or_else(|| std::env::var("TELLERS_API_KEY").ok())
        .ok_or_else(|| "TELLERS_API_KEY not set".to_string())
}

pub fn get_bearer_header(auth_bearer_arg: Option<String>) -> Option<String> {
    let bearer_env = auth_bearer_arg
        .or_else(|| std::env::var("TELLERS_AUTH_BEARER").ok())
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

