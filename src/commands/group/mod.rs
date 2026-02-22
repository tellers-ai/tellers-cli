use clap::{Args, Subcommand};

mod list;

pub use list::{run as list_run, ListArgs};

#[derive(Args, Debug)]
pub struct GroupArgs {
    #[command(subcommand)]
    pub command: GroupCommand,
}

#[derive(Subcommand, Debug)]
pub enum GroupCommand {
    List(ListArgs),
}

