use clap::{Parser, Subcommand};

use crate::commands::asset::AssetArgs;
use crate::commands::project::ProjectArgs;
use crate::commands::task::TaskArgs;
use crate::commands::upload::UploadArgs;

#[derive(Parser, Debug)]
#[command(name = "tellers", version, about = "Tellers CLI", long_about = None)]
pub struct Cli {
    #[arg(long)]
    pub full_auto: bool,

    #[arg(long)]
    pub background: bool,

    /// Disable interaction with the agent (single response, no REPL).
    #[arg(long)]
    pub no_interaction: bool,

    /// Print the turn's parsed result as JSON: projects, assets and summary
    /// (no_interaction implied).
    #[arg(long)]
    pub json_response: bool,

    /// Tool(s) to enable (can be repeated). Omit for default tools.
    #[arg(long = "tool", value_name = "TOOL_ID")]
    pub tools: Vec<String>,

    /// LLM model to use (e.g. gpt-5.4-2026-03-05).
    #[arg(long, value_name = "MODEL")]
    pub llm_model: Option<String>,

    /// Have the agent do the whole job, export the project(s) to MP4, and report
    /// them on a TELLERS_EXPORT line; then download each into DIR as
    /// "<project name>.mp4" (no_interaction implied).
    #[arg(long, value_name = "DIR")]
    pub export_to: Option<std::path::PathBuf>,

    /// Rendition(s) for --export-to (360p, 480p, 720p, 1080p, 1440p, 4k). Default 1080p.
    #[arg(long = "export-rendition", value_name = "RESOLUTION", requires = "export_to")]
    pub export_renditions: Vec<String>,

    /// Replace existing files in the --export-to directory.
    #[arg(long, requires = "export_to")]
    pub export_force: bool,

    /// Print the instructions --export-to appends to the prompt (to stderr).
    #[arg(long, requires = "export_to")]
    pub show_instructions: bool,

    /// Interactively set json_response, no_interaction, tools, and llm_model.
    #[arg(short, long)]
    pub interactive: bool,

    #[arg()]
    pub prompt: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Sign in with a Tellers account in the browser.
    Login,
    /// Remove the saved Tellers login.
    Logout,
    /// Check whether a newer tellers release is available.
    Update,
    Asset(AssetArgs),
    Project(ProjectArgs),
    Task(TaskArgs),
    Upload(UploadArgs),
}

impl Cli {
    pub fn parse() -> Self {
        <Self as clap::Parser>::parse()
    }

    pub fn print_help_and_exit() -> ! {
        use clap::CommandFactory;
        let mut cmd = Self::command();
        let _ = cmd.print_help();
        println!();
        std::process::exit(0);
    }
}
