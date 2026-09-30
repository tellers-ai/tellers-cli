use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use futures_util::{StreamExt, TryStreamExt};
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use crate::commands::api_config;
use crate::output;

/// OTIO file bundle layout (see OpenTimelineIO `otioz` adapter).
const OTIOZ_VERSION: &str = "1.0.0";
const OTIOZ_CONTENT_FILE: &str = "content.otio";
const OTIOZ_VERSION_FILE: &str = "version.txt";
const OTIOZ_MEDIA_DIR: &str = "media";

const PRESIGN_EXPIRES_MIN_SEC: u32 = 3_600;
const PRESIGN_EXPIRES_MAX_SEC: u32 = 604_800;
const PARALLEL_MEDIA_DOWNLOADS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum OtioFormat {
    /// Timeline JSON; clips point at remote media URLs.
    Otio,
    /// Zip bundle with the timeline and all referenced media.
    Otioz,
}

impl OtioFormat {
    fn extension(self) -> &'static str {
        match self {
            OtioFormat::Otio => "otio",
            OtioFormat::Otioz => "otioz",
        }
    }

    fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "otio" => Some(OtioFormat::Otio),
            "otioz" => Some(OtioFormat::Otioz),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum OtioRendition {
    #[value(name = "480p")]
    P480,
    #[value(name = "720p")]
    P720,
    #[value(name = "1080p")]
    P1080,
    Original,
    Highest,
    Lowest,
}

impl OtioRendition {
    fn as_query(self) -> &'static str {
        match self {
            OtioRendition::P480 => "480p",
            OtioRendition::P720 => "720p",
            OtioRendition::P1080 => "1080p",
            OtioRendition::Original => "original",
            OtioRendition::Highest => "highest",
            OtioRendition::Lowest => "lowest",
        }
    }
}

#[derive(Args, Debug)]
pub struct ExportOtioArgs {
    /// Project ID to export
    #[arg(value_name = "PROJECT_ID")]
    pub project_id: String,

    /// Local destination. Defaults to <PROJECT_ID>.<format> in the current directory.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Output format. Inferred from the --output extension when omitted, otherwise otio.
    #[arg(long, value_enum)]
    pub format: Option<OtioFormat>,

    /// Media rendition referenced by (or bundled into) the timeline.
    #[arg(long, value_enum, default_value = "highest")]
    pub rendition: OtioRendition,

    /// Lifetime in seconds of the presigned media URLs (3600 to 604800).
    #[arg(long, value_name = "SECONDS", default_value_t = 43_200)]
    pub presign_expires_in: u32,

    /// Write clip URLs as authenticated /asset/url/{asset_id} redirects instead of
    /// presigned URLs (otio only).
    #[arg(long)]
    pub redirect_urls: bool,

    /// Replace an existing destination file.
    #[arg(long)]
    pub force: bool,

    #[arg(long, env = "TELLERS_AUTH_BEARER", hide = true)]
    pub auth_bearer: Option<String>,
}

pub fn run(args: ExportOtioArgs) -> Result<(), String> {
    let format = resolve_format(args.format, args.output.as_deref())?;
    if format == OtioFormat::Otioz && args.redirect_urls {
        return Err("--redirect-urls is only supported with --format otio".to_string());
    }
    if !(PRESIGN_EXPIRES_MIN_SEC..=PRESIGN_EXPIRES_MAX_SEC).contains(&args.presign_expires_in) {
        return Err(format!(
            "--presign-expires-in must be between {} and {} seconds",
            PRESIGN_EXPIRES_MIN_SEC, PRESIGN_EXPIRES_MAX_SEC
        ));
    }

    let destination = args
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("{}.{}", args.project_id, format.extension())));
    if destination.exists() && !args.force {
        return Err(format!(
            "destination already exists: {}; use --force to replace it",
            destination.display()
        ));
    }

    // This endpoint only accepts user sessions, not API keys.
    let bearer_header =
        api_config::get_bearer_header(args.auth_bearer.clone()).ok_or_else(|| {
            "OTIO export requires a user session; run `tellers login` or set TELLERS_AUTH_BEARER"
                .to_string()
        })?;

    tokio::runtime::Runtime::new()
        .map_err(|e| format!("failed to start runtime: {}", e))?
        .block_on(async move {
            let client = reqwest::Client::new();
            let otio = fetch_otio(&client, &args, &bearer_header).await?;
            let partial = partial_path(&destination);
            let result = match format {
                OtioFormat::Otio => write_otio(&otio, &partial).await,
                OtioFormat::Otioz => write_otioz(&client, otio, &destination, &partial).await,
            };
            let result = match result {
                Ok(()) => finalize(&partial, &destination, args.force).await,
                Err(e) => Err(e),
            };
            if result.is_err() {
                let _ = tokio::fs::remove_file(&partial).await;
            }
            result?;
            println!(
                "exported {} to {}",
                format.extension(),
                destination.display()
            );
            Ok(())
        })
}

fn resolve_format(
    explicit: Option<OtioFormat>,
    output: Option<&Path>,
) -> Result<OtioFormat, String> {
    let inferred = output.and_then(OtioFormat::from_path);
    match (explicit, inferred) {
        (Some(f), Some(i)) if f != i => Err(format!(
            "--format {} does not match the output extension .{}",
            f.extension(),
            i.extension()
        )),
        (Some(f), _) => Ok(f),
        (None, Some(i)) => Ok(i),
        (None, None) => Ok(OtioFormat::Otio),
    }
}

async fn fetch_otio(
    client: &reqwest::Client,
    args: &ExportOtioArgs,
    bearer_header: &str,
) -> Result<Value, String> {
    let url = format!(
        "{}/project/{}/export_tellers_otio",
        api_config::get_api_base().trim_end_matches('/'),
        url::form_urlencoded::byte_serialize(args.project_id.as_bytes()).collect::<String>()
    );
    let response = client
        .get(&url)
        .header(reqwest::header::AUTHORIZATION, bearer_header)
        .query(&[
            ("input_rendition", args.rendition.as_query().to_string()),
            ("presign_expires_in", args.presign_expires_in.to_string()),
            ("use_redirect_urls", args.redirect_urls.to_string()),
        ])
        .send()
        .await
        .map_err(|e| format!("OTIO export request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let mut message = format!("OTIO export failed; http_status: {}", status);
        if !body.is_empty() {
            message.push_str(&format!("; response: {}", body));
        }
        return Err(message);
    }
    response
        .json::<Value>()
        .await
        .map_err(|e| format!("failed to decode OTIO export: {}", e))
}

async fn write_otio(otio: &Value, partial: &Path) -> Result<(), String> {
    let json =
        serde_json::to_vec_pretty(otio).map_err(|e| format!("failed to encode OTIO: {}", e))?;
    tokio::fs::write(partial, json)
        .await
        .map_err(|e| format!("failed to write {}: {}", partial.display(), e))
}

async fn write_otioz(
    client: &reqwest::Client,
    mut otio: Value,
    destination: &Path,
    partial: &Path,
) -> Result<(), String> {
    let mut media: Vec<BundledMedia> = Vec::new();
    let mut skipped = 0usize;
    collect_remote_media(&mut otio, &mut media, &mut HashMap::new(), &mut skipped);
    if skipped > 0 {
        output::warning(format!(
            "{} media reference(s) have no downloadable URL and were left unchanged",
            skipped
        ));
    }

    let staging = staging_dir(destination);
    tokio::fs::create_dir_all(&staging)
        .await
        .map_err(|e| format!("failed to create {}: {}", staging.display(), e))?;

    let result = async {
        download_media(client, &media, &staging).await?;
        let content = serde_json::to_vec_pretty(&otio)
            .map_err(|e| format!("failed to encode OTIO: {}", e))?;
        let partial = partial.to_path_buf();
        let staging = staging.clone();
        tokio::task::spawn_blocking(move || build_bundle(&partial, &content, &media, &staging))
            .await
            .map_err(|e| format!("bundle task failed: {}", e))?
    }
    .await;

    let _ = tokio::fs::remove_dir_all(&staging).await;
    result
}

struct BundledMedia {
    url: String,
    file_name: String,
}

/// Rewrites every remote `target_url` to its path inside the bundle, recording what to download.
/// Walks the whole document so clips nested in stacks are included.
fn collect_remote_media(
    value: &mut Value,
    media: &mut Vec<BundledMedia>,
    by_url: &mut HashMap<String, String>,
    skipped: &mut usize,
) {
    match value {
        Value::Object(map) => {
            let is_external_ref = map
                .get("OTIO_SCHEMA")
                .and_then(Value::as_str)
                .is_some_and(|s| s.starts_with("ExternalReference."));
            if is_external_ref {
                let url = map
                    .get("target_url")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                match url {
                    Some(url) if url.starts_with("http://") || url.starts_with("https://") => {
                        let file_name = match by_url.get(&url) {
                            Some(name) => name.clone(),
                            None => {
                                let name = unique_file_name(
                                    &media_file_stem(map, &url),
                                    &url_extension(&url),
                                    media,
                                );
                                by_url.insert(url.clone(), name.clone());
                                media.push(BundledMedia {
                                    url,
                                    file_name: name.clone(),
                                });
                                name
                            }
                        };
                        map.insert(
                            "target_url".to_string(),
                            Value::String(format!("{}/{}", OTIOZ_MEDIA_DIR, file_name)),
                        );
                    }
                    _ => *skipped += 1,
                }
            }
            for child in map.values_mut() {
                collect_remote_media(child, media, by_url, skipped);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_remote_media(item, media, by_url, skipped);
            }
        }
        _ => {}
    }
}

fn media_file_stem(reference: &serde_json::Map<String, Value>, url: &str) -> String {
    let metadata = reference.get("metadata");
    let media_id = metadata
        .and_then(|m| m.get("tellers.ai"))
        .and_then(|t| t.get("media_id"))
        .or_else(|| metadata.and_then(|m| m.get("media_id")))
        .and_then(Value::as_str);
    let stem = media_id
        .map(str::to_string)
        .or_else(|| {
            url::Url::parse(url).ok().and_then(|u| {
                u.path_segments()
                    .and_then(|mut s| s.next_back().map(str::to_string))
                    .map(|name| match name.rsplit_once('.') {
                        Some((stem, _)) => stem.to_string(),
                        None => name,
                    })
            })
        })
        .unwrap_or_default();
    let sanitized: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "media".to_string()
    } else {
        sanitized
    }
}

fn url_extension(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.path_segments()
                .and_then(|mut s| s.next_back().map(str::to_string))
        })
        .and_then(|name| {
            name.rsplit_once('.')
                .map(|(_, ext)| ext.to_ascii_lowercase())
        })
        .filter(|ext| {
            !ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .map(|ext| format!(".{}", ext))
        .unwrap_or_default()
}

fn unique_file_name(stem: &str, ext: &str, existing: &[BundledMedia]) -> String {
    let taken = |name: &str| existing.iter().any(|m| m.file_name == name);
    let mut name = format!("{}{}", stem, ext);
    let mut n = 1;
    while taken(&name) {
        name = format!("{}_{}{}", stem, n, ext);
        n += 1;
    }
    name
}

async fn download_media(
    client: &reqwest::Client,
    media: &[BundledMedia],
    staging: &Path,
) -> Result<(), String> {
    let total = media.len();
    futures_util::stream::iter(media.iter().enumerate())
        .map(|(i, item)| async move {
            output::info(format!(
                "downloading media {}/{}: {}",
                i + 1,
                total,
                item.file_name
            ));
            download_to(client, &item.url, &staging.join(&item.file_name)).await
        })
        .buffer_unordered(PARALLEL_MEDIA_DOWNLOADS)
        .try_collect::<()>()
        .await
}

async fn download_to(client: &reqwest::Client, url: &str, path: &Path) -> Result<(), String> {
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("media download failed: {}", e.without_url()))?;
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| format!("failed to create {}: {}", path.display(), e))?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("media download failed: {}", e.without_url()))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("failed writing {}: {}", path.display(), e))?;
    }
    file.flush()
        .await
        .map_err(|e| format!("failed flushing {}: {}", path.display(), e))
}

fn build_bundle(
    partial: &Path,
    content: &[u8],
    media: &[BundledMedia],
    staging: &Path,
) -> Result<(), String> {
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    let file = std::fs::File::create(partial)
        .map_err(|e| format!("failed to create {}: {}", partial.display(), e))?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let zip_err = |e: zip::result::ZipError| format!("failed writing {}: {}", partial.display(), e);
    let io_err = |e: std::io::Error| format!("failed writing {}: {}", partial.display(), e);

    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    // Media is already compressed; store it as-is, like OpenTimelineIO does.
    let stored = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(true);

    zip.start_file(OTIOZ_VERSION_FILE, deflated)
        .map_err(zip_err)?;
    zip.write_all(OTIOZ_VERSION.as_bytes()).map_err(io_err)?;
    zip.start_file(OTIOZ_CONTENT_FILE, deflated)
        .map_err(zip_err)?;
    zip.write_all(content).map_err(io_err)?;
    for item in media {
        let source = staging.join(&item.file_name);
        let mut reader = std::fs::File::open(&source)
            .map_err(|e| format!("failed to open {}: {}", source.display(), e))?;
        zip.start_file(format!("{}/{}", OTIOZ_MEDIA_DIR, item.file_name), stored)
            .map_err(zip_err)?;
        std::io::copy(&mut reader, &mut zip).map_err(io_err)?;
    }
    zip.finish().map_err(zip_err)?.flush().map_err(io_err)
}

fn partial_path(destination: &Path) -> PathBuf {
    let mut partial = destination.as_os_str().to_os_string();
    partial.push(".part");
    PathBuf::from(partial)
}

fn staging_dir(destination: &Path) -> PathBuf {
    let mut staging = destination.as_os_str().to_os_string();
    staging.push(".media.part");
    PathBuf::from(staging)
}

async fn finalize(partial: &Path, destination: &Path, force: bool) -> Result<(), String> {
    if destination.exists() {
        if !force {
            return Err(format!(
                "destination appeared during export: {}; use --force to replace it",
                destination.display()
            ));
        }
        tokio::fs::remove_file(destination)
            .await
            .map_err(|e| format!("failed replacing {}: {}", destination.display(), e))?;
    }
    tokio::fs::rename(partial, destination)
        .await
        .map_err(|e| format!("failed finalizing {}: {}", destination.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn format_is_inferred_from_extension() {
        assert_eq!(resolve_format(None, None), Ok(OtioFormat::Otio));
        assert_eq!(
            resolve_format(None, Some(Path::new("a/b.OTIOZ"))),
            Ok(OtioFormat::Otioz)
        );
        assert_eq!(
            resolve_format(Some(OtioFormat::Otioz), Some(Path::new("b.zip"))),
            Ok(OtioFormat::Otioz)
        );
        assert!(resolve_format(Some(OtioFormat::Otio), Some(Path::new("b.otioz"))).is_err());
    }

    #[test]
    fn remote_media_is_rewritten_and_deduplicated() {
        let clip = |url: &str, id: &str| {
            json!({
                "OTIO_SCHEMA": "Clip.2",
                "media_references": {"DEFAULT_MEDIA": {
                    "OTIO_SCHEMA": "ExternalReference.1",
                    "target_url": url,
                    "metadata": {"tellers.ai": {"media_id": id}}
                }}
            })
        };
        let mut otio = json!({"tracks": {"children": [{"children": [
            clip("https://s3/x/a.MP4?sig=1", "asset-1"),
            clip("https://s3/x/a.MP4?sig=1", "asset-1"),
            clip("https://s3/y/b?sig=2", "asset-1"),
            clip("clip_name.mov", "asset-3"),
            {"OTIO_SCHEMA": "Stack.1", "children": [clip("https://s3/z/c.mxf", "asset/4")]}
        ]}]}});

        let mut media = Vec::new();
        let mut skipped = 0;
        collect_remote_media(&mut otio, &mut media, &mut HashMap::new(), &mut skipped);

        let names: Vec<_> = media.iter().map(|m| m.file_name.as_str()).collect();
        assert_eq!(names, ["asset-1.mp4", "asset-1", "asset_4.mxf"]);
        assert_eq!(skipped, 1);
        let target = |i: usize| {
            otio["tracks"]["children"][0]["children"][i]["media_references"]["DEFAULT_MEDIA"]
                ["target_url"]
                .clone()
        };
        assert_eq!(target(0), "media/asset-1.mp4");
        assert_eq!(target(1), "media/asset-1.mp4");
        assert_eq!(target(2), "media/asset-1");
        assert_eq!(target(3), "clip_name.mov");
    }
}
