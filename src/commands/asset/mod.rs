use clap::{Args, Subcommand};

mod list;
mod set_anonymous_read;

pub use list::{run as list_run, ListArgs};
pub use set_anonymous_read::{run as set_anonymous_read_run, SetAnonymousReadArgs};

#[derive(Args, Debug)]
pub struct AssetArgs {
    #[command(subcommand)]
    pub command: AssetCommand,
}

#[derive(Subcommand, Debug)]
pub enum AssetCommand {
    List(ListArgs),
    /// Set anonymous read permission for an asset (allow unauthenticated read).
    SetAnonymousRead(SetAnonymousReadArgs),
}

