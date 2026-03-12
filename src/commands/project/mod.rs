use clap::{Args, Subcommand};

mod export;

pub use export::{run as export_run, ExportArgs};

#[derive(Args, Debug)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub command: ProjectCommand,
}

#[derive(Subcommand, Debug)]
pub enum ProjectCommand {
    /// Export project to MP4 at one or more resolutions (optionally into a project folder).
    Export(ExportArgs),
}
