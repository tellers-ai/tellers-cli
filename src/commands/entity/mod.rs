use clap::{Args, Subcommand};

mod create;

pub use create::{run as create_run, CreateArgs};

#[derive(Args, Debug)]
pub struct EntityArgs {
    #[command(subcommand)]
    pub command: EntityCommand,
}

#[derive(Subcommand, Debug)]
pub enum EntityCommand {
    Create(CreateArgs),
}

