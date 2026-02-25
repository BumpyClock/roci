//! Roci CLI binary entry point.

use clap::Parser;
use roci::cli::{AuthCommands, Cli, Commands};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Auth(auth_args) => match auth_args.command {
            AuthCommands::Login(args) => roci::cli::auth::handle_login(&args.provider).await,
            AuthCommands::Status => roci::cli::auth::handle_status().await,
            AuthCommands::Logout(args) => roci::cli::auth::handle_logout(&args.provider).await,
        },
        Commands::Chat(_chat_args) => {
            eprintln!("Chat command not yet implemented. Use the library API or examples.");
            std::process::exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
