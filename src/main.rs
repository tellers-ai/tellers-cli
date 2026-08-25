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
                commands::asset::AssetCommand::Download(args) => {
                    if let Err(error) = commands::asset::download_run(args) {
                        eprintln!("error: {}", error);
                        std::process::exit(1);
                    }
                }
                commands::asset::AssetCommand::SetAnonymousRead(args) => {
                    if let Err(error) = commands::asset::set_anonymous_read_run(args) {
                        eprintln!("error: {}", error);
                        std::process::exit(1);
                    }
                }
            }
        }
        Some(cli::Command::Project(project_args)) => {
            match project_args.command {
                commands::project::ProjectCommand::Export(export_args) => {
                    if let Err(error) = commands::project::export_run(export_args) {
                        eprintln!("error: {}", error);
                        std::process::exit(1);
                    }
                }
            }
        }
        Some(cli::Command::Task(task_args)) => {
            if let Err(error) = commands::task::run(task_args) {
                eprintln!("error: {}", error);
                std::process::exit(1);
            }
        }
        Some(cli::Command::Upload(upload_args)) => {
            let suppress_plain_error = matches!(
                &upload_args.command,
                commands::upload::UploadCommand::Upload(args) if args.machine_readable
            );
            if let Err(error) = commands::upload::run(upload_args) {
                if !suppress_plain_error {
                    eprintln!("error: {}", error);
                }
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
