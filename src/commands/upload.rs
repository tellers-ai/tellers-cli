use clap::Args;

#[derive(Args, Debug)]
pub struct UploadArgs {
    /// Only upload proxies/metadata for the media (no full content)
    #[arg(long, default_value_t = false)]
    pub only_proxies: bool,

    /// Path to media folder to upload
    pub path: String,
}

pub fn run(args: UploadArgs) -> Result<(), String> {
    // Placeholder: Implement scanning `args.path`, respecting `args.only_proxies`, and uploading via API
    println!(
        "Uploading from '{}' (only_proxies = {})",
        args.path, args.only_proxies
    );
    Ok(())
}


