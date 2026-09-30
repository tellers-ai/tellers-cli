use clap::{Args, Subcommand};

mod export;
mod export_otio;

pub use export::{run as export_run, ExportArgs};
pub use export_otio::{run as export_otio_run, ExportOtioArgs};

#[derive(Args, Debug)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub command: ProjectCommand,
}

#[derive(Subcommand, Debug)]
pub enum ProjectCommand {
    /// Export project to MP4 at one or more resolutions (optionally into a project folder).
    Export(ExportArgs),
    /// Export the project timeline as OTIO JSON (.otio) or an OTIO bundle with media (.otioz).
    ExportOtio(ExportOtioArgs),
}
