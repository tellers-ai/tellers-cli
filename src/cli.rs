use clap::{Parser, Subcommand};

use crate::commands::list::ListArgs;
use crate::commands::upload::UploadArgs;

#[derive(Parser, Debug)]
#[command(name = "tellers", version, about = "Tellers CLI", long_about = None)]
pub struct Cli {
    #[arg(long)]
    pub full_auto: bool,

    #[arg(long)]
    pub background: bool,

    #[arg()]
    pub prompt: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    List(ListArgs),
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
