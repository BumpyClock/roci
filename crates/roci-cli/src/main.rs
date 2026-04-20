//! Roci CLI binary entry point.

mod audio_cmd;
mod chat;
mod cli;
mod errors;
mod skills_cmd;

use clap::Parser;

use cli::{AudioCommands, AuthCommands, Cli, Commands};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Auth(auth_args) => match auth_args.command {
            AuthCommands::Login(args) => cli::auth::handle_login(&args.provider).await,
            AuthCommands::Status => cli::auth::handle_status().await,
            AuthCommands::Logout(args) => cli::auth::handle_logout(&args.provider).await,
        },
        Commands::Audio(audio_args) => match audio_args.command {
            AudioCommands::Transcribe(args) => audio_cmd::handle_transcribe(args).await,
            AudioCommands::Speak(args) => audio_cmd::handle_speak(args).await,
        },
        Commands::Chat(chat_args) => chat::handle_chat(chat_args).await,
        Commands::Skills(skills_args) => skills_cmd::handle_skills(skills_args).await,
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
