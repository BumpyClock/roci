//! Roci CLI binary entry point.

mod chat;
mod cli;
mod errors;
mod skills_cmd;

use clap::Parser;

use cli::{AuthCommands, Cli, Commands};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Auth(auth_args) => match auth_args.command {
            AuthCommands::Login(args) => cli::auth::handle_login(&args.provider).await,
            AuthCommands::Status => cli::auth::handle_status().await,
            AuthCommands::Logout(args) => cli::auth::handle_logout(&args.provider).await,
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
