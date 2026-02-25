//! CLI entry point for Roci.

pub mod auth;

use clap::{Parser, Subcommand};

/// Roci AI CLI
#[derive(Parser, Debug)]
#[command(name = "roci", version, about = "Roci — Rust AI SDK CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

/// Top-level CLI commands.
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Authentication management
    Auth(AuthArgs),
    /// Chat with an AI model
    Chat(ChatArgs),
}

/// Arguments for the `auth` subcommand group.
#[derive(Parser, Debug)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub command: AuthCommands,
}

/// Auth subcommands for login, status, and logout.
#[derive(Subcommand, Debug)]
pub enum AuthCommands {
    /// Login to a provider
    Login(LoginArgs),
    /// Show authentication status
    Status,
    /// Logout from a provider
    Logout(LogoutArgs),
}

/// Arguments for `roci auth login`.
#[derive(Parser, Debug)]
pub struct LoginArgs {
    /// Provider to login to (copilot, chatgpt, claude)
    pub provider: String,
}

/// Arguments for `roci auth logout`.
#[derive(Parser, Debug)]
pub struct LogoutArgs {
    /// Provider to logout from (copilot, chatgpt, claude)
    pub provider: String,
}

/// Arguments for the `chat` subcommand.
#[derive(Parser, Debug)]
pub struct ChatArgs {
    /// Model to use (format: provider:model, e.g., openai:gpt-4o)
    #[arg(short, long, default_value = "openai:gpt-4o")]
    pub model: String,

    /// System prompt
    #[arg(short, long)]
    pub system: Option<String>,

    /// Temperature (0.0 - 2.0)
    #[arg(short, long)]
    pub temperature: Option<f64>,

    /// Max tokens
    #[arg(long)]
    pub max_tokens: Option<u32>,

    /// Enable streaming output
    #[arg(long, default_value = "true")]
    pub stream: bool,

    /// User prompt (positional)
    pub prompt: Option<String>,
}

impl Cli {
    /// Parse CLI arguments.
    pub fn parse_args() -> Self {
        Self::parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_auth_login_copilot() {
        let cli = Cli::try_parse_from(["roci", "auth", "login", "copilot"]).unwrap();
        match cli.command {
            Commands::Auth(auth) => match auth.command {
                AuthCommands::Login(args) => assert_eq!(args.provider, "copilot"),
                other => panic!("expected Login, got {other:?}"),
            },
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn parse_auth_status() {
        let cli = Cli::try_parse_from(["roci", "auth", "status"]).unwrap();
        match cli.command {
            Commands::Auth(auth) => {
                assert!(matches!(auth.command, AuthCommands::Status));
            }
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn parse_auth_logout_claude() {
        let cli = Cli::try_parse_from(["roci", "auth", "logout", "claude"]).unwrap();
        match cli.command {
            Commands::Auth(auth) => match auth.command {
                AuthCommands::Logout(args) => assert_eq!(args.provider, "claude"),
                other => panic!("expected Logout, got {other:?}"),
            },
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn parse_chat_with_defaults() {
        let cli = Cli::try_parse_from(["roci", "chat"]).unwrap();
        match cli.command {
            Commands::Chat(args) => {
                assert_eq!(args.model, "openai:gpt-4o");
                assert!(args.system.is_none());
                assert!(args.temperature.is_none());
                assert!(args.max_tokens.is_none());
                assert!(args.stream);
                assert!(args.prompt.is_none());
            }
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn parse_chat_with_all_options() {
        let cli = Cli::try_parse_from([
            "roci",
            "chat",
            "-m",
            "anthropic:claude-4-sonnet",
            "-s",
            "You are helpful",
            "-t",
            "0.7",
            "--max-tokens",
            "1024",
            "Hello world",
        ])
        .unwrap();
        match cli.command {
            Commands::Chat(args) => {
                assert_eq!(args.model, "anthropic:claude-4-sonnet");
                assert_eq!(args.system.as_deref(), Some("You are helpful"));
                assert!((args.temperature.unwrap() - 0.7).abs() < f64::EPSILON);
                assert_eq!(args.max_tokens, Some(1024));
                assert!(args.stream);
                assert_eq!(args.prompt.as_deref(), Some("Hello world"));
            }
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn parse_missing_subcommand_is_error() {
        assert!(Cli::try_parse_from(["roci"]).is_err());
    }

    #[test]
    fn parse_auth_login_missing_provider_is_error() {
        assert!(Cli::try_parse_from(["roci", "auth", "login"]).is_err());
    }
}
