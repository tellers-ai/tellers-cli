mod auth;
mod cli;
mod commands;
mod logger;
mod media;
mod output;
mod tui;
mod uploads_tracking;

use cli::Cli;

fn main() {
    if let Err(e) = logger::init() {
        eprintln!("Warning: Failed to initialize logger: {}", e);
    }

    let cli = Cli::parse();

    match cli.command {
        Some(cli::Command::Asset(asset_args)) => {
            match asset_args.command {
                commands::asset::AssetCommand::List(list_args) => {
                    if let Err(error) = commands::asset::list_run(list_args) {
                        eprintln!("error: {}", error);
                        std::process::exit(1);
                    }
                }
                commands::asset::AssetCommand::Preprocess(preprocess_args) => {
                    if let Err(error) = commands::asset::preprocess_run(preprocess_args) {
                        eprintln!("error: {}", error);
                        std::process::exit(1);
                    }
                }
            }
        }
        Some(cli::Command::Entity(entity_args)) => {
            match entity_args.command {
                commands::entity::EntityCommand::Create(create_args) => {
                    if let Err(error) = commands::entity::create_run(create_args) {
                        eprintln!("error: {}", error);
                        std::process::exit(1);
                    }
                }
            }
        }
        Some(cli::Command::Group(_group_args)) => {
            eprintln!("Group commands are not yet available. Please add group endpoints to the OpenAPI spec first.");
            std::process::exit(1);
        }
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
                } else if let Err(error) =
                    commands::prompt::run_interactive(prompt_text, cli.full_auto)
                {
                    eprintln!("error: {}", error);
                    std::process::exit(1);
                }
            } else {
                // No subcommand and no prompt provided
                Cli::print_help_and_exit();
            }
        }
    }
}
