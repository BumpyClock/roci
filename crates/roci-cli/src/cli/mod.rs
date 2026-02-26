//! CLI argument definitions for Roci.

pub mod auth;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Roci AI CLI
#[derive(Parser, Debug)]
#[command(name = "roci-agent", version, about = "Roci — Rust AI SDK CLI")]
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
    /// Manage installed skills
    Skills(SkillsArgs),
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

/// Arguments for `roci-agent auth login`.
#[derive(Parser, Debug)]
pub struct LoginArgs {
    /// Provider to login to (copilot, codex, claude)
    pub provider: String,
}

/// Arguments for `roci-agent auth logout`.
#[derive(Parser, Debug)]
pub struct LogoutArgs {
    /// Provider to logout from (copilot, codex, claude)
    pub provider: String,
}

/// Arguments for the `chat` subcommand.
#[derive(Parser, Debug)]
pub struct ChatArgs {
    /// Model to use (format: provider:model, e.g., openai:gpt-4o or codex:gpt-5.3-codex-spark)
    #[arg(short, long, default_value = "openai:gpt-4o")]
    pub model: String,

    /// System prompt
    #[arg(short, long)]
    pub system: Option<String>,

    /// Temperature (0.0 - 2.0)
    #[arg(short, long)]
    pub temperature: Option<f64>,

    /// Explicit skill path (file or directory)
    #[arg(long, value_name = "PATH")]
    pub skill_path: Vec<PathBuf>,

    /// Additional skill root directory (searched after default roots)
    #[arg(long, value_name = "PATH")]
    pub skill_root: Vec<PathBuf>,

    /// Disable skill loading
    #[arg(long)]
    pub no_skills: bool,

    /// Max tokens
    #[arg(long)]
    pub max_tokens: Option<u32>,

    /// Enable streaming output
    #[arg(long, default_value = "true")]
    pub stream: bool,

    /// User prompt (positional)
    pub prompt: Option<String>,
}

/// Arguments for the `skills` subcommand group.
#[derive(Parser, Debug)]
pub struct SkillsArgs {
    #[command(subcommand)]
    pub command: SkillsCommands,
}

/// Skill management subcommands.
#[derive(Subcommand, Debug)]
pub enum SkillsCommands {
    /// Install one or more skills from a source
    Install(InstallSkillArgs),
    /// Remove one managed skill
    Remove(RemoveSkillArgs),
    /// Update one or all managed skills
    Update(UpdateSkillsArgs),
    /// List discovered and managed skills
    List,
}

/// Arguments for `roci-agent skills install`.
#[derive(Parser, Debug)]
pub struct InstallSkillArgs {
    /// Skill source (local path or git URL)
    pub source: String,

    /// Use project-local scope (.roci/skills) instead of global scope
    #[arg(long)]
    pub local: bool,
}

/// Arguments for `roci-agent skills remove`.
#[derive(Parser, Debug)]
pub struct RemoveSkillArgs {
    /// Managed skill name
    pub name: String,

    /// Use project-local scope (.roci/skills) instead of global scope
    #[arg(long)]
    pub local: bool,
}

/// Arguments for `roci-agent skills update`.
#[derive(Parser, Debug)]
pub struct UpdateSkillsArgs {
    /// Optional managed skill name; omit to update all managed skills in scope
    pub name: Option<String>,

    /// Use project-local scope (.roci/skills) instead of global scope
    #[arg(long)]
    pub local: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_auth_login_copilot() {
        let cli = Cli::try_parse_from(["roci-agent", "auth", "login", "copilot"]).unwrap();
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
        let cli = Cli::try_parse_from(["roci-agent", "auth", "status"]).unwrap();
        match cli.command {
            Commands::Auth(auth) => {
                assert!(matches!(auth.command, AuthCommands::Status));
            }
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn parse_auth_logout_claude() {
        let cli = Cli::try_parse_from(["roci-agent", "auth", "logout", "claude"]).unwrap();
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
        let cli = Cli::try_parse_from(["roci-agent", "chat"]).unwrap();
        match cli.command {
            Commands::Chat(args) => {
                assert_eq!(args.model, "openai:gpt-4o");
                assert!(args.system.is_none());
                assert!(args.temperature.is_none());
                assert!(args.skill_path.is_empty());
                assert!(args.skill_root.is_empty());
                assert!(!args.no_skills);
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
            "roci-agent",
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
                assert!(args.skill_path.is_empty());
                assert!(args.skill_root.is_empty());
                assert!(!args.no_skills);
                assert_eq!(args.max_tokens, Some(1024));
                assert!(args.stream);
                assert_eq!(args.prompt.as_deref(), Some("Hello world"));
            }
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn parse_chat_with_no_skills() {
        let cli = Cli::try_parse_from(["roci-agent", "chat", "--no-skills", "prompt"]).unwrap();
        match cli.command {
            Commands::Chat(args) => {
                assert!(args.no_skills);
                assert_eq!(args.prompt.as_deref(), Some("prompt"));
                assert!(args.skill_path.is_empty());
                assert!(args.skill_root.is_empty());
            }
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn parse_chat_with_multiple_skill_paths() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "chat",
            "--skill-path",
            "./skills/global",
            "--skill-path",
            "./skills/project",
            "Hi",
        ])
        .unwrap();
        match cli.command {
            Commands::Chat(args) => {
                assert_eq!(
                    args.skill_path,
                    vec![
                        PathBuf::from("./skills/global"),
                        PathBuf::from("./skills/project")
                    ]
                );
            }
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn parse_chat_with_multiple_skill_roots() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "chat",
            "--skill-root",
            "./root/one",
            "--skill-root",
            "./root/two",
            "Hi",
        ])
        .unwrap();
        match cli.command {
            Commands::Chat(args) => {
                assert_eq!(
                    args.skill_root,
                    vec![PathBuf::from("./root/one"), PathBuf::from("./root/two")]
                );
            }
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn parse_missing_subcommand_is_error() {
        assert!(Cli::try_parse_from(["roci-agent"]).is_err());
    }

    #[test]
    fn parse_auth_login_missing_provider_is_error() {
        assert!(Cli::try_parse_from(["roci-agent", "auth", "login"]).is_err());
    }

    #[test]
    fn parse_skills_install_with_local_scope() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "skills",
            "install",
            "https://example.com/repo.git",
            "--local",
        ])
        .unwrap();
        match cli.command {
            Commands::Skills(skills) => match skills.command {
                SkillsCommands::Install(args) => {
                    assert_eq!(args.source, "https://example.com/repo.git");
                    assert!(args.local);
                }
                other => panic!("expected Install, got {other:?}"),
            },
            other => panic!("expected Skills, got {other:?}"),
        }
    }

    #[test]
    fn parse_skills_remove_global_scope_by_default() {
        let cli = Cli::try_parse_from(["roci-agent", "skills", "remove", "my-skill"]).unwrap();
        match cli.command {
            Commands::Skills(skills) => match skills.command {
                SkillsCommands::Remove(args) => {
                    assert_eq!(args.name, "my-skill");
                    assert!(!args.local);
                }
                other => panic!("expected Remove, got {other:?}"),
            },
            other => panic!("expected Skills, got {other:?}"),
        }
    }

    #[test]
    fn parse_skills_update_without_name() {
        let cli = Cli::try_parse_from(["roci-agent", "skills", "update", "--local"]).unwrap();
        match cli.command {
            Commands::Skills(skills) => match skills.command {
                SkillsCommands::Update(args) => {
                    assert!(args.name.is_none());
                    assert!(args.local);
                }
                other => panic!("expected Update, got {other:?}"),
            },
            other => panic!("expected Skills, got {other:?}"),
        }
    }

    #[test]
    fn parse_skills_update_with_name() {
        let cli = Cli::try_parse_from(["roci-agent", "skills", "update", "alpha"]).unwrap();
        match cli.command {
            Commands::Skills(skills) => match skills.command {
                SkillsCommands::Update(args) => {
                    assert_eq!(args.name.as_deref(), Some("alpha"));
                    assert!(!args.local);
                }
                other => panic!("expected Update, got {other:?}"),
            },
            other => panic!("expected Skills, got {other:?}"),
        }
    }

    #[test]
    fn parse_skills_list() {
        let cli = Cli::try_parse_from(["roci-agent", "skills", "list"]).unwrap();
        match cli.command {
            Commands::Skills(skills) => {
                assert!(matches!(skills.command, SkillsCommands::List));
            }
            other => panic!("expected Skills, got {other:?}"),
        }
    }
}
