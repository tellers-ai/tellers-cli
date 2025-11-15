use clap::{Args, Subcommand};

mod create;
mod list;

pub use create::{run as create_run, CreateArgs};
pub use list::{run as list_run, ListArgs};

#[derive(Args, Debug)]
pub struct GroupArgs {
    #[command(subcommand)]
    pub command: GroupCommand,
}

#[derive(Subcommand, Debug)]
pub enum GroupCommand {
    List(ListArgs),
    Create(CreateArgs),
}

