use clap::{Args, Subcommand};

mod list;

pub use list::{run as list_run, ListArgs};

#[derive(Args, Debug)]
pub struct AssetArgs {
    #[command(subcommand)]
    pub command: AssetCommand,
}

#[derive(Subcommand, Debug)]
pub enum AssetCommand {
    List(ListArgs),
}

