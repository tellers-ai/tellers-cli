use clap::Args;
use regex::Regex;
use tellers_api_client::apis::accepts_api_key_api as api;
use tellers_api_client::apis::configuration::Configuration;
use tellers_api_client::models::FileReference;

#[derive(Args, Debug)]
pub struct ListArgs {
    /// Path to list files from
    pub path: String,

    /// Regex pattern to filter file names
    #[arg(long)]
    pub regex: Option<String>,

    /// Minimum duration in seconds (inclusive)
    #[arg(long)]
    pub min_duration: Option<f64>,

    /// Maximum duration in seconds (inclusive)
    #[arg(long)]
    pub max_duration: Option<f64>,

    /// Only show files without duration (null duration)
    #[arg(long, default_value_t = false)]
    pub no_duration: bool,

    /// Maximum number of results to return
    #[arg(long, default_value_t = 100)]
    pub limit: i32,

    /// Page number (0-indexed)
    #[arg(long, default_value_t = 0)]
    pub page: i32,

    /// Only output file IDs, one per line
    #[arg(long, default_value_t = false)]
    pub only_id: bool,

    /// API key (can also be set via TELLERS_API_KEY env var)
    #[arg(long, env = "TELLERS_API_KEY")]
    pub api_key: Option<String>,

    /// Bearer token (can also be set via TELLERS_AUTH_BEARER env var)
    #[arg(long, env = "TELLERS_AUTH_BEARER")]
    pub auth_bearer: Option<String>,
}

pub fn run(args: ListArgs) -> Result<(), String> {
    let api_base = std::env::var("TELLERS_API_BASE")
        .unwrap_or_else(|_| "https://api.prod.aws.tellers.ai".to_string());
    let api_key = args
        .api_key
        .or_else(|| std::env::var("TELLERS_API_KEY").ok())
        .ok_or_else(|| "TELLERS_API_KEY not set".to_string())?;

    let mut cfg = Configuration::default();
    cfg.base_path = api_base;

    let bearer_env = args
        .auth_bearer
        .or_else(|| std::env::var("TELLERS_AUTH_BEARER").ok())
        .filter(|v| !v.is_empty());
    let bearer_header = bearer_env.as_deref().map(|b| {
        if b.starts_with("Bearer ") {
            b.to_string()
        } else {
            format!("Bearer {}", b)
        }
    });

    // Compile regex if provided
    let regex_pattern = if let Some(ref pattern) = args.regex {
        Some(Regex::new(pattern).map_err(|e| format!("Invalid regex pattern: {}", e))?)
    } else {
        None
    };

    let page = args.page;
    let limit = args.limit;

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            // Fetch files from API
            let files = api::request_files_processing_users_sources_list_files_get(
                &cfg,
                &args.path,
                limit,
                page,
                None, // folder_on_top
                None, // with_public_assets
                Some(&api_key),
                bearer_header.as_deref(),
            )
            .await
            .map_err(|e| {
                let mut m = format!("failed to list files: {}", e);
                match &e {
                    tellers_api_client::apis::Error::Reqwest(req_err) => {
                        if let Some(status) = req_err.status() {
                            m.push_str(&format!("; http_status: {}", status));
                        }
                    }
                    tellers_api_client::apis::Error::ResponseError(resp) => {
                        m.push_str(&format!("; http_status: {}", resp.status));
                        if !resp.content.is_empty() {
                            m.push_str(&format!("; response_body: {}", resp.content));
                        }
                    }
                    _ => {}
                }
                m
            })?;

            // Apply filters
            let mut filtered_files: Vec<&FileReference> = files.iter().collect();

            // Apply regex filter
            if let Some(ref regex) = regex_pattern {
                filtered_files.retain(|file| regex.is_match(&file.file_name));
            }

            // Apply duration filters
            if args.min_duration.is_some() || args.max_duration.is_some() {
                filtered_files.retain(|file| {
                    if let Some(Some(duration)) = file.duration_seconds {
                        let duration_f64: f64 = duration;
                        let min_ok = args.min_duration.map_or(true, |min| duration_f64 >= min);
                        let max_ok = args.max_duration.map_or(true, |max| duration_f64 <= max);
                        min_ok && max_ok
                    } else {
                        // If duration is None, exclude from duration filtering
                        // Only include if no duration filters are specified
                        args.min_duration.is_none() && args.max_duration.is_none()
                    }
                });
            }

            // Apply no-duration filter (only show files without duration)
            if args.no_duration {
                filtered_files.retain(|file| {
                    // Keep only files where duration_seconds is None or Some(None)
                    matches!(file.duration_seconds, None | Some(None))
                });
            }

            // Apply limit to results
            let mut result_files: Vec<&FileReference> = filtered_files.into_iter().collect();
            if result_files.len() > limit as usize {
                result_files.truncate(limit as usize);
            }

            // Output results
            if args.only_id {
                for file in result_files {
                    if let Some(ref file_id) = file.file_id {
                        println!("{}", file_id);
                    }
                }
            } else {
                // Calculate column widths
                let mut id_width = 2; // "ID"
                let mut name_width = 4; // "Name"
                let mut type_width = 4; // "Type"
                let mut duration_width = 8; // "Duration"
                let mut category_width = 8; // "Category"

                // Prepare data and calculate widths
                let mut rows: Vec<(String, String, String, String, String)> = Vec::new();
                for file in &result_files {
                    let duration_str = file
                        .duration_seconds
                        .flatten()
                        .map(|d: f64| format!("{:.2}s", d))
                        .unwrap_or_else(|| "N/A".to_string());
                    let file_type_str = file
                        .file_type
                        .as_ref()
                        .map(|s| s.as_str())
                        .unwrap_or("N/A")
                        .to_string();
                    let file_id_str = file
                        .file_id
                        .as_ref()
                        .map(|s| s.as_str())
                        .unwrap_or("N/A")
                        .to_string();
                    let category_str = if file.is_folder { "folder" } else { "file" }.to_string();

                    id_width = id_width.max(file_id_str.len());
                    name_width = name_width.max(file.file_name.len());
                    type_width = type_width.max(file_type_str.len());
                    duration_width = duration_width.max(duration_str.len());
                    category_width = category_width.max(category_str.len());

                    rows.push((file_id_str, file.file_name.clone(), file_type_str, duration_str, category_str));
                }

                // Print header with proper spacing
                println!(
                    "{:<id_w$} | {:<name_w$} | {:<type_w$} | {:<duration_w$} | {:<category_w$}",
                    "ID",
                    "Name",
                    "Type",
                    "Duration",
                    "Category",
                    id_w = id_width,
                    name_w = name_width,
                    type_w = type_width,
                    duration_w = duration_width,
                    category_w = category_width
                );

                // Print separator line
                println!(
                    "{:-<id_w$}-+-{:-<name_w$}-+-{:-<type_w$}-+-{:-<duration_w$}-+-{:-<category_w$}-",
                    "",
                    "",
                    "",
                    "",
                    "",
                    id_w = id_width,
                    name_w = name_width,
                    type_w = type_width,
                    duration_w = duration_width,
                    category_w = category_width
                );

                // Print data rows
                for (file_id_str, file_name, file_type_str, duration_str, category_str) in rows {
                    println!(
                        "{:<id_w$} | {:<name_w$} | {:<type_w$} | {:<duration_w$} | {:<category_w$}",
                        file_id_str,
                        file_name,
                        file_type_str,
                        duration_str,
                        category_str,
                        id_w = id_width,
                        name_w = name_width,
                        type_w = type_width,
                        duration_w = duration_width,
                        category_w = category_width
                    );
                }
            }

            Ok(())
        })
}

