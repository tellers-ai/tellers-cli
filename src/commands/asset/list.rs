use clap::Args;
use regex::Regex;
use tellers_api_client::apis::accepts_api_key_api as api;
use tellers_api_client::models::FileReference;

use crate::commands::api_config;

#[derive(Args, Debug)]
pub struct ListArgs {
    pub path: String,

    #[arg(long)]
    pub regex: Option<String>,

    #[arg(long)]
    pub min_duration: Option<f64>,

    #[arg(long)]
    pub max_duration: Option<f64>,

    #[arg(long, default_value_t = false)]
    pub no_duration: bool,

    #[arg(long, default_value_t = 100)]
    pub limit: i32,

    #[arg(long, default_value_t = 0)]
    pub page: i32,

    #[arg(long, default_value_t = false)]
    pub only_id: bool,

    #[arg(long, env = "TELLERS_API_KEY")]
    pub api_key: Option<String>,

    #[arg(long, env = "TELLERS_AUTH_BEARER")]
    pub auth_bearer: Option<String>,
}

pub fn run(args: ListArgs) -> Result<(), String> {
    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(args.api_key)?;
    let bearer_header = api_config::get_bearer_header(args.auth_bearer);

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
            let files = api::request_files_processing_users_sources_list_files_get(
                &cfg,
                &args.path,
                limit,
                page,
                None,
                None,
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

            let mut filtered_files: Vec<&FileReference> = files.iter().collect();
            if let Some(ref regex) = regex_pattern {
                filtered_files.retain(|file| regex.is_match(&file.file_name));
            }

            if args.min_duration.is_some() || args.max_duration.is_some() {
                filtered_files.retain(|file| {
                    if let Some(Some(duration)) = file.duration_seconds {
                        let duration_f64: f64 = duration;
                        let min_ok = args.min_duration.map_or(true, |min| duration_f64 >= min);
                        let max_ok = args.max_duration.map_or(true, |max| duration_f64 <= max);
                        min_ok && max_ok
                    } else {
                        args.min_duration.is_none() && args.max_duration.is_none()
                    }
                });
            }

            if args.no_duration {
                filtered_files.retain(|file| {
                    matches!(file.duration_seconds, None | Some(None))
                });
            }

            let mut result_files: Vec<&FileReference> = filtered_files.into_iter().collect();
            if result_files.len() > limit as usize {
                result_files.truncate(limit as usize);
            }

            if args.only_id {
                for file in result_files {
                    if let Some(ref file_id) = file.file_id {
                        println!("{}", file_id);
                    }
                }
            } else {
                let mut id_width = 2;
                let mut name_width = 4;
                let mut type_width = 4;
                let mut duration_width = 8;
                let mut category_width = 8;

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

