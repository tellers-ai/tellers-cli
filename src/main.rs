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
            }
        }
        Some(cli::Command::Upload(upload_args)) => {
            if let Err(error) = commands::upload::run(upload_args) {
                eprintln!("error: {}", error);
                std::process::exit(1);
            }
        }
        None => {
            if let Some(prompt_text) = cli.prompt {
                let mut opts = commands::prompt::PromptOptions::from_cli(
                    cli.no_interaction,
                    cli.json_response,
                    cli.tools.clone(),
                    cli.llm_model.clone(),
                );
                if cli.interactive {
                    match commands::prompt::run_interactive_options(&opts) {
                        Ok(interactive_opts) => opts = interactive_opts,
                        Err(error) => {
                            eprintln!("error: {}", error);
                            std::process::exit(1);
                        }
                    }
                }
                if cli.background {
                    match commands::prompt::run_background(prompt_text, cli.full_auto, opts) {
                        Ok(result) => println!("{}", result),
                        Err(error) => {
                            eprintln!("error: {}", error);
                            std::process::exit(1);
                        }
                    }
                } else if let Err(error) =
                    commands::prompt::run_interactive(prompt_text, cli.full_auto, opts)
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
