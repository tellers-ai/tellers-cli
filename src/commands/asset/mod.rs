use clap::{Args, Subcommand};

mod list;
mod preprocess;

pub use list::{run as list_run, ListArgs};
pub use preprocess::{run as preprocess_run, PreprocessArgs};

#[derive(Args, Debug)]
pub struct AssetArgs {
    #[command(subcommand)]
    pub command: AssetCommand,
}

#[derive(Subcommand, Debug)]
pub enum AssetCommand {
    List(ListArgs),
    Preprocess(PreprocessArgs),
}

