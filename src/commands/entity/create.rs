use clap::Args;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use tellers_api_client::apis::accepts_api_key_api as api;
use tellers_api_client::models::{
    AssetUploadRequest, AssetUploadResponse, CreateEntityRequest, ProcessEntityRequest,
    SourceFileInfo,
};
use uuid::Uuid;

use crate::auth;
use crate::commands::api_config;
use crate::media::metadata::extract_media_metadata;
use crate::output;
use crate::uploads_tracking;

#[derive(Args, Debug)]
pub struct CreateArgs {
    #[arg(long)]
    pub group_id: String,

    #[arg(long)]
    pub name: String,

    #[arg(long)]
    pub asset_id: Option<String>,

    #[arg(long)]
    pub filepath: Option<String>,

    #[arg(long, default_value = "")]
    pub description: String,

    #[arg(long, env = "TELLERS_API_KEY")]
    pub api_key: Option<String>,

    #[arg(long, env = "TELLERS_AUTH_BEARER")]
    pub auth_bearer: Option<String>,
}

pub fn run(args: CreateArgs) -> Result<(), String> {
    if args.asset_id.is_some() && args.filepath.is_some() {
        return Err("Cannot specify both --asset-id and --filepath. Use only one.".to_string());
    }

    let cfg = api_config::create_config();
    let api_key = api_config::get_api_key(args.api_key)?;
    let bearer_header = api_config::get_bearer_header(args.auth_bearer.clone());

    let user_id = auth::get_user_id_from_bearer(bearer_header.as_deref());

    let asset_id = if let Some(ref filepath) = args.filepath {
        let file_path = PathBuf::from(filepath);
        if !file_path.exists() {
            return Err(format!("File not found: {}", filepath));
        }

        match uploads_tracking::get_asset_id_from_path(&user_id, &file_path)? {
            Some(id) => {
                output::info(format!("Found asset_id {} for file {}", id, filepath));
                Some(id)
            }
            None => {
                output::info(format!("File {} not in upload history, uploading now...", filepath));
                Some(upload_file_and_get_asset_id(
                    &file_path,
                    &cfg,
                    &api_key,
                    bearer_header.as_deref(),
                    &user_id,
                )?)
            }
        }
    } else {
        args.asset_id
    };

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            let create_req = CreateEntityRequest::new(
                args.group_id.clone(),
                args.name.clone(),
                args.description.clone(),
            );

            output::info(format!("Creating entity: name={}, group_id={}", args.name, args.group_id));

            let create_resp = api::create_entity_users_entity_create_post(
                &cfg,
                create_req,
                Some(&api_key),
                bearer_header.as_deref(),
            )
            .await
            .map_err(|e| {
                let mut m = format!("failed to create entity: {}", e);
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

            let entity_id = create_resp.entity_id;
            output::success(format!("Entity created successfully: {}", entity_id));

            if let Some(asset_id) = asset_id {
                output::info(format!("Associating asset {} with entity {}", asset_id, entity_id));

                let asset = AssetUploadResponse::new(
                    "".to_string(),
                    "".to_string(),
                    asset_id.clone(),
                );

                let process_req = ProcessEntityRequest::new(entity_id.clone(), vec![asset]);

                let process_resp = api::process_entity_users_entity_preprocess_post(
                    &cfg,
                    process_req,
                    None,
                    Some(&api_key),
                    bearer_header.as_deref(),
                )
                .await
                .map_err(|e| {
                    let mut m = format!("failed to process entity with asset: {}", e);
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

                output::success(format!(
                    "Asset {} associated with entity {} (task_id: {})",
                    asset_id, entity_id, process_resp.task_id
                ));
            }

            Ok(())
        })
}

fn upload_file_and_get_asset_id(
    file_path: &PathBuf,
    cfg: &tellers_api_client::apis::configuration::Configuration,
    api_key: &str,
    bearer_header: Option<&str>,
    user_id: &str,
) -> Result<String, String> {
    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            let content_length = std::fs::metadata(file_path)
                .map_err(|e| format!("failed to stat {}: {}", file_path.display(), e))?
                .len();

            let upload_id = Uuid::new_v4().to_string();
            let upload_request_id = Uuid::new_v4().to_string();

            let file_name_str = file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let in_app_path = file_name_str.clone();

            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i32;

            let umid = extract_media_metadata(file_path).ok();
            let mut source_info = SourceFileInfo::new(
                "__user_upload__".to_string(),
                None,
                None,
                vec!["__current_user__".to_string()],
                Some(now_secs),
                now_secs,
                vec![in_app_path.clone()],
                Some(file_name_str),
                None,
                vec![],
            );

            if let Some(metadata) = umid {
                if let Some(umid_value) = metadata.material_package_umid {
                    source_info.capture_device_umid = Some(Some(umid_value));
                }
                if let Some(first_umid) = metadata.file_package_umids.first() {
                    source_info.umid = Some(Some(first_umid.clone()));
                }
            }

            let upload_req = AssetUploadRequest::new(
                i32::try_from(content_length).unwrap_or(i32::MAX),
                upload_id.clone(),
                source_info,
            );

            output::info(format!("Requesting presigned URL for {}", file_path.display()));

            let mut responses = api::create_upload_urls_users_assets_upload_urls_post(
                cfg,
                vec![upload_req],
                Some(api_key),
                bearer_header,
            )
            .await
            .map_err(|e| {
                let mut m = format!("failed to get upload url: {}", e);
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

            if responses.is_empty() {
                return Err("No upload response received".to_string());
            }

            let upload_resp = responses.remove(0);
            let upload_url = upload_resp.presigned_put_url.clone();
            let asset_id = upload_resp.asset_id.clone();

            output::info(format!("Uploading file to presigned URL..."));

            let mut f = File::open(file_path)
                .map_err(|e| format!("failed to open {}: {}", file_path.display(), e))?;
            let mut buf = Vec::with_capacity(content_length as usize);

            const CHUNK_SIZE: usize = 1024 * 1024;
            let mut chunk = vec![0u8; CHUNK_SIZE.min(content_length as usize)];

            loop {
                let n = f
                    .read(&mut chunk)
                    .map_err(|e| format!("failed to read {}: {}", file_path.display(), e))?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }

            let content_type = mime_guess::from_path(file_path)
                .first_or_text_plain()
                .essence_str()
                .to_string();

            let http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .map_err(|e| format!("failed to build http client: {}", e))?;

            let put_res = http
                .put(upload_url)
                .header(reqwest::header::CONTENT_LENGTH, content_length)
                .header(reqwest::header::CONTENT_TYPE, &content_type)
                .body(buf)
                .send()
                .await
                .map_err(|e| format!("upload failed for {}: {}", file_path.display(), e))?;

            if !put_res.status().is_success() {
                let status = put_res.status();
                let body = put_res
                    .text()
                    .await
                    .unwrap_or_else(|_| "<failed to read error body>".to_string());
                return Err(format!(
                    "Upload failed for {}: HTTP {} - {}",
                    file_path.display(),
                    status,
                    body
                ));
            }

            if let Err(e) = uploads_tracking::record_upload(
                user_id,
                file_path,
                &in_app_path,
                &asset_id,
                &upload_request_id,
            ) {
                output::warning(format!("Failed to record upload in tracking file: {}", e));
            }

            output::success(format!("File uploaded successfully, asset_id: {}", asset_id));

            Ok(asset_id)
        })
}

