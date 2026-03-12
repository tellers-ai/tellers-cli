use clap::{Parser, Subcommand};

use crate::commands::asset::AssetArgs;
use crate::commands::project::ProjectArgs;
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

    /// Use JSON response endpoint (no_interaction implied; SSE tellers.json_result events).
    #[arg(long)]
    pub json_response: bool,

    /// Tool(s) to enable (can be repeated). Omit for default tools.
    #[arg(long = "tool", value_name = "TOOL_ID")]
    pub tools: Vec<String>,

    /// LLM model to use (e.g. gpt-5.4-2026-03-05).
    #[arg(long, value_name = "MODEL")]
    pub llm_model: Option<String>,

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
    Asset(AssetArgs),
    Project(ProjectArgs),
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
