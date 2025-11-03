mod auth;
mod cli;
mod commands;
mod media;
mod uploads_tracking;

use cli::Cli;

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(cli::Command::Upload(upload_args)) => {
            if let Err(error) = commands::upload::run(upload_args) {
                eprintln!("error: {}", error);
                std::process::exit(1);
            }
        }
        None => {
            if let Some(prompt_text) = cli.prompt {
                if cli.background {
                    match commands::prompt::run_background(prompt_text, cli.full_auto) {
                        Ok(chat_id) => {
                            // Only print chat id in background mode
                            println!("{}", chat_id);
                        }
                        Err(error) => {
                            eprintln!("error: {}", error);
                            std::process::exit(1);
                        }
                    }
                } else {
                    if let Err(error) =
                        commands::prompt::run_interactive(prompt_text, cli.full_auto)
                    {
                        eprintln!("error: {}", error);
                        std::process::exit(1);
                    }
                }
            } else {
                // No subcommand and no prompt provided
                Cli::print_help_and_exit();
            }
        }
    }
}
