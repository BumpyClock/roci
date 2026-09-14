//! Roci CLI binary entry point.

mod audio_cmd;
mod chat;
mod cli;
mod errors;
mod models_cmd;
mod session_cmd;
mod skills_cmd;
mod tool_contracts_smoke;

use clap::Parser;

use cli::{AudioCommands, AuthCommands, Cli, Commands};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Auth(auth_args) => match auth_args.command {
            AuthCommands::Login(args) => {
                cli::auth::handle_login(
                    &args.provider,
                    &auth_args.account,
                    args.flow.map(Into::into),
                )
                .await
            }
            AuthCommands::Import(args) => {
                cli::auth::handle_import(&args.provider, &auth_args.account).await
            }
            AuthCommands::Status(args) => {
                cli::auth::handle_status(args.json, &auth_args.account).await
            }
            AuthCommands::Logout(args) => {
                cli::auth::handle_logout(&args.provider, &auth_args.account).await
            }
            AuthCommands::Configure(args) => {
                cli::auth::handle_configure(
                    &args.provider,
                    args.endpoint.as_deref(),
                    &auth_args.account,
                )
                .await
            }
            AuthCommands::Providers(args) => {
                cli::auth::handle_providers(args.json, &auth_args.account).await
            }
        },
        Commands::Audio(audio_args) => match audio_args.command {
            AudioCommands::Transcribe(args) => audio_cmd::handle_transcribe(args).await,
            AudioCommands::Speak(args) => audio_cmd::handle_speak(args).await,
        },
        Commands::Chat(chat_args) => chat::handle_chat(chat_args).await,
        Commands::Models(models_args) => models_cmd::handle_models(models_args).await,
        Commands::Session(session_args) => session_cmd::handle_session(session_args).await,
        Commands::Skills(skills_args) => skills_cmd::handle_skills(skills_args).await,
        Commands::ToolContractsSmoke(args) => {
            tool_contracts_smoke::handle_tool_contracts_smoke(args).await
        }
    };

    if let Err(error) = result {
        let message = if let Some(roci_error) = error.downcast_ref::<roci::error::RociError>() {
            errors::format_error_help(roci_error)
        } else {
            format!("{error}")
        };

        eprintln!("Error: {message}");
        std::process::exit(1);
    }
}
