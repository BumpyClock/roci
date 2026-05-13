//! CLI argument definitions for Roci.

pub mod auth;

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// Roci AI CLI
#[derive(Parser, Debug)]
#[command(name = "roci-agent", version, about = "Roci — Rust AI SDK CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

/// Top-level CLI commands.
#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Commands {
    /// Authentication management
    Auth(AuthArgs),
    /// OpenAI-backed file and stdio audio commands
    Audio(AudioArgs),
    /// Chat with an AI model
    Chat(ChatArgs),
    /// Inspect available models
    Models(ModelsArgs),
    /// Manage durable agent sessions
    Session(SessionArgs),
    /// Manage installed skills
    Skills(SkillsArgs),
    /// Exercise tool contract behavior against a live provider.
    #[command(hide = true)]
    ToolContractsSmoke(ToolContractsSmokeArgs),
}

/// Arguments for hidden `roci-agent tool-contracts-smoke`.
#[derive(Parser, Debug)]
pub struct ToolContractsSmokeArgs {
    /// Model to use as provider:model.
    #[arg(long, value_name = "PROVIDER:MODEL")]
    pub model: String,

    /// Provider endpoint/base URL override.
    #[arg(long)]
    pub endpoint: Option<String>,

    /// Provider API key override.
    #[arg(long)]
    pub api_key: Option<String>,

    /// Smoke case to run.
    #[arg(long, value_enum, default_value_t = ToolContractsSmokeCaseArg::All)]
    pub case: ToolContractsSmokeCaseArg,
}

/// Hidden tool-contract smoke case selector.
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum ToolContractsSmokeCaseArg {
    All,
    Result,
    Prompt,
    Alias,
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

/// Arguments for the `audio` subcommand group.
#[derive(Parser, Debug)]
pub struct AudioArgs {
    #[command(subcommand)]
    pub command: AudioCommands,
}

/// Audio subcommands.
#[derive(Subcommand, Debug)]
pub enum AudioCommands {
    /// Transcribe audio bytes from a file or stdin to text
    Transcribe(TranscribeArgs),
    /// Generate speech audio from text into a file or stdout
    Speak(SpeakArgs),
}

/// Arguments for `roci-agent audio transcribe`.
#[derive(Parser, Debug)]
pub struct TranscribeArgs {
    /// Input audio file path, or '-' to read audio bytes from stdin
    #[arg(long, value_name = "PATH")]
    pub input: PathBuf,

    /// Override MIME type (required when reading from stdin)
    #[arg(long, value_name = "MIME")]
    pub mime_type: Option<String>,

    /// Language hint (for example: en)
    #[arg(long, value_name = "LANG")]
    pub language: Option<String>,

    /// Transcription model
    #[arg(long, default_value = "whisper-1")]
    pub model: String,

    /// Print the full transcription result as JSON
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `roci-agent audio speak`.
#[derive(Parser, Debug)]
pub struct SpeakArgs {
    /// Output file path, or '-' to write audio bytes to stdout
    #[arg(long, value_name = "PATH")]
    pub output: PathBuf,

    /// OpenAI voice id
    #[arg(long, default_value = "alloy")]
    pub voice: String,

    /// Output audio format
    #[arg(long, default_value = "mp3")]
    pub format: AudioFormatArg,

    /// Speech speed multiplier (finite value from 0.25 to 4.0 inclusive)
    #[arg(long, value_parser = parse_speech_speed)]
    pub speed: Option<f64>,

    /// OpenAI speech model
    #[arg(long, default_value = "tts-1")]
    pub model: String,

    /// Input text
    pub text: String,
}

/// CLI-local audio format values.
#[derive(Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum AudioFormatArg {
    Mp3,
    Opus,
    Aac,
    Flac,
    Wav,
    Pcm16,
}

fn parse_speech_speed(value: &str) -> Result<f64, String> {
    let speed = value
        .parse::<f64>()
        .map_err(|_| "speech speed must be a finite number between 0.25 and 4.0".to_string())?;
    if (0.25..=4.0).contains(&speed) && speed.is_finite() {
        Ok(speed)
    } else {
        Err("speech speed must be a finite number between 0.25 and 4.0".to_string())
    }
}

fn parse_max_retry_attempts(value: &str) -> Result<u32, String> {
    let attempts = value
        .parse::<u32>()
        .map_err(|_| "max-retry-attempts must be >= 1".to_string())?;
    if attempts >= 1 {
        Ok(attempts)
    } else {
        Err("max-retry-attempts must be >= 1".to_string())
    }
}

fn parse_positive_usize(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| "must be >= 1".to_string())?;
    if parsed >= 1 {
        Ok(parsed)
    } else {
        Err("must be >= 1".to_string())
    }
}

/// Arguments for the `chat` subcommand.
#[derive(Parser, Debug)]
pub struct ChatArgs {
    /// Model to use (format: provider:model, e.g., openai:gpt-4o or codex:gpt-5.3-codex-spark)
    #[arg(short, long, default_value = "openai:gpt-4o")]
    pub model: String,

    /// Additional fallback model candidate to try after the primary model. Repeatable.
    #[arg(long = "candidate-model", value_name = "PROVIDER:MODEL")]
    pub candidate_models: Vec<String>,

    /// Retry mode for provider failures.
    #[arg(long = "retry-mode", value_enum, default_value_t = ChatRetryModeArg::Bounded)]
    pub retry_mode: ChatRetryModeArg,

    /// Maximum attempts per candidate when --retry-mode=bounded. Includes the first attempt.
    #[arg(long = "max-retry-attempts", default_value_t = 3, value_parser = parse_max_retry_attempts)]
    pub max_retry_attempts: u32,

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

    /// Select main/default agent profile by id.
    #[arg(long = "agent", value_name = "PROFILE")]
    pub agent: Option<String>,

    /// Disable subagent routing tools.
    #[arg(long)]
    pub no_subagents: bool,

    /// List loaded agent profiles and exit.
    #[arg(long)]
    pub list_agents: bool,

    /// Disable all model-visible tools
    #[arg(long)]
    pub no_tools: bool,

    /// Allow only this tool name. Repeatable.
    #[arg(long = "tool", value_name = "NAME")]
    pub tools: Vec<String>,

    /// Exclude this tool name. Repeatable.
    #[arg(long = "exclude-tool", value_name = "NAME")]
    pub exclude_tools: Vec<String>,

    /// Max tokens
    #[arg(long)]
    pub max_tokens: Option<u32>,

    /// Override provider context window (input+output token limit)
    #[arg(long, value_name = "TOKENS", value_parser = parse_positive_usize)]
    pub context_window_override: Option<usize>,

    /// Reserve tokens for model output during budget checks
    #[arg(long = "reserve-output-tokens", value_name = "TOKENS", value_parser = parse_positive_usize)]
    pub reserve_output_tokens: Option<usize>,

    /// Max input tokens allowed for the current turn
    #[arg(long = "max-turn-input-tokens", value_name = "TOKENS", value_parser = parse_positive_usize)]
    pub max_turn_input_tokens: Option<usize>,

    /// Max cumulative input tokens for this session
    #[arg(
        long = "max-session-input-tokens",
        value_name = "TOKENS",
        value_parser = parse_positive_usize
    )]
    pub max_session_input_tokens: Option<usize>,

    /// Max cumulative output tokens for this session
    #[arg(
        long = "max-session-output-tokens",
        value_name = "TOKENS",
        value_parser = parse_positive_usize
    )]
    pub max_session_output_tokens: Option<usize>,

    /// Disable auto compaction summary+pruning
    #[arg(long = "no-auto-compaction")]
    pub no_auto_compaction: bool,

    /// Compaction reserve token target
    #[arg(
        long = "compaction-reserve-tokens",
        value_name = "TOKENS",
        value_parser = parse_positive_usize
    )]
    pub compaction_reserve_tokens: Option<usize>,

    /// Compaction keep-recent token floor
    #[arg(
        long = "compaction-keep-recent-tokens",
        value_name = "TOKENS",
        value_parser = parse_positive_usize
    )]
    pub compaction_keep_recent_tokens: Option<usize>,

    /// Compaction model override
    #[arg(long = "compaction-model", value_name = "PROVIDER:MODEL")]
    pub compaction_model: Option<String>,

    /// Tool approval behavior
    #[arg(long, value_enum, default_value_t = ChatApprovalArg::Ask)]
    pub approval: ChatApprovalArg,

    /// Durable session root directory. When set, chat events/resources are stored under <root>/<session-id>.
    #[arg(long, value_name = "PATH")]
    pub session_root: Option<PathBuf>,

    /// Durable session id to use with --session-root. Defaults to a new UUID when omitted.
    #[arg(long, value_name = "ID", requires = "session_root")]
    pub session_id: Option<String>,

    /// File attachment to include with the prompt. Repeatable.
    #[arg(long = "attach", value_name = "PATH")]
    pub attachments: Vec<PathBuf>,

    /// MCP stdio server spec (repeatable). Format: `key=value` pairs separated by commas.
    /// Keys: `id`, `label`, `command`, `arg` (repeat for multiple args).
    /// Example: `--mcp-stdio 'id=local,label=Local Files,command=npx,arg=-y,arg=@modelcontextprotocol/server-filesystem,arg=.'`
    #[arg(long = "mcp-stdio", value_name = "SPEC")]
    pub mcp_stdio: Vec<String>,

    /// MCP streamable HTTP server spec (repeatable). Format: `key=value` pairs separated by commas.
    /// Keys: `id`, `label`, `url`, `auth_token`, `header` (`header` value uses `Name:Value`; repeatable).
    /// Example: `--mcp-streamable-http 'id=remote,label=Remote Docs,url=http://localhost:3000/mcp,header=x-env:dev'`
    #[arg(long = "mcp-streamable-http", value_name = "SPEC")]
    pub mcp_streamable_http: Vec<String>,

    /// MCP WebSocket server spec (repeatable). Format: `key=value` pairs separated by commas.
    /// Keys: `id`, `label`, `url`, `auth_token`, `header` (`header` value uses `Name:Value`; repeatable).
    /// Example: `--mcp-websocket 'id=remote,label=Remote WS,url=ws://localhost:3000/mcp,header=x-env:dev'`
    #[arg(long = "mcp-websocket", value_name = "SPEC")]
    pub mcp_websocket: Vec<String>,

    /// User prompt (positional)
    pub prompt: Option<String>,
}

/// CLI-local tool approval policy values.
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum ChatApprovalArg {
    Ask,
    Always,
    Never,
}

/// CLI-local retry mode values for chat.
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum ChatRetryModeArg {
    Bounded,
    Persistent,
}

/// Arguments for the `models` subcommand group.
#[derive(Parser, Debug)]
pub struct ModelsArgs {
    #[command(subcommand)]
    pub command: ModelsCommands,
}

/// Model catalog subcommands.
#[derive(Subcommand, Debug)]
pub enum ModelsCommands {
    /// List available models
    List(ModelsListArgs),
    /// Exercise runtime model switching without starting a provider call.
    #[command(hide = true)]
    SwitchSmoke(ModelsSwitchSmokeArgs),
    /// Exercise runtime model switching and send a prompt through the switched model.
    #[command(hide = true)]
    SwitchChatSmoke(ModelsSwitchChatSmokeArgs),
}

/// Arguments for `roci-agent models list`.
#[derive(Parser, Debug)]
pub struct ModelsListArgs {
    /// Provider key to list models for
    #[arg(long, value_name = "PROVIDER")]
    pub provider: Option<String>,

    /// Print models as JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for hidden `roci-agent models switch-smoke`.
#[derive(Parser, Debug)]
pub struct ModelsSwitchSmokeArgs {
    /// Initial runtime model as provider:model.
    #[arg(long, value_name = "MODEL")]
    pub from: String,

    /// Replacement runtime model as provider:model.
    #[arg(long, value_name = "MODEL")]
    pub to: String,

    /// Print switch result as JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for hidden `roci-agent models switch-chat-smoke`.
#[derive(Parser, Debug)]
pub struct ModelsSwitchChatSmokeArgs {
    /// Initial runtime model as provider:model.
    #[arg(long, value_name = "MODEL")]
    pub from: String,

    /// Replacement runtime model as provider:model.
    #[arg(long, value_name = "MODEL")]
    pub to: String,

    /// Prompt sent after switching.
    #[arg(long, value_name = "TEXT")]
    pub prompt: String,

    /// Required substring in the assistant response.
    #[arg(long, value_name = "TEXT")]
    pub expect: Option<String>,

    /// Print result as JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for the `session` subcommand group.
#[derive(Parser, Debug)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub command: SessionCommands,
}

/// Durable session subcommands.
#[derive(Subcommand, Debug)]
pub enum SessionCommands {
    /// Create a durable session
    Create(SessionCreateArgs),
    /// List durable sessions
    List(SessionListArgs),
    /// Delete a durable session
    Delete(SessionDeleteArgs),
    /// Export a durable session snapshot
    Export(SessionExportArgs),
    /// Import a durable session snapshot
    Import(SessionImportArgs),
    /// Recover a durable session snapshot from local state
    RecoverExport(SessionRecoverExportArgs),
    /// Import a recovered durable session snapshot into local state
    RecoverImport(SessionRecoverImportArgs),
}

/// Arguments for `roci-agent session create`.
#[derive(Parser, Debug)]
pub struct SessionCreateArgs {
    /// Session root directory. Defaults to the app data session directory.
    #[arg(long, value_name = "PATH")]
    pub root: Option<PathBuf>,

    /// Session id. Defaults to a generated UUID.
    #[arg(long, value_name = "ID")]
    pub id: Option<String>,

    /// Human-readable session title.
    #[arg(long)]
    pub title: Option<String>,

    /// Print a JSON summary.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `roci-agent session list`.
#[derive(Parser, Debug)]
pub struct SessionListArgs {
    /// Session root directory. Defaults to the app data session directory.
    #[arg(long, value_name = "PATH")]
    pub root: Option<PathBuf>,

    /// Print sessions as JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `roci-agent session delete`.
#[derive(Parser, Debug)]
pub struct SessionDeleteArgs {
    /// Session root directory. Defaults to the app data session directory.
    #[arg(long, value_name = "PATH")]
    pub root: Option<PathBuf>,

    /// Session id to delete.
    pub id: String,
}

/// Arguments for `roci-agent session export`.
#[derive(Parser, Debug)]
pub struct SessionExportArgs {
    /// Session root directory. Defaults to the app data session directory.
    #[arg(long, value_name = "PATH")]
    pub root: Option<PathBuf>,

    /// Session id to export.
    pub id: String,

    /// Output snapshot JSON path.
    #[arg(long, value_name = "PATH")]
    pub output: PathBuf,

    /// Print a JSON summary.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `roci-agent session import`.
#[derive(Parser, Debug)]
pub struct SessionImportArgs {
    /// Session root directory. Defaults to the app data session directory.
    #[arg(long, value_name = "PATH")]
    pub root: Option<PathBuf>,

    /// Input snapshot JSON path.
    #[arg(long, value_name = "PATH")]
    pub input: PathBuf,

    /// Imported session id. Defaults to a generated UUID.
    #[arg(long, value_name = "ID")]
    pub id: Option<String>,

    /// Print a JSON summary.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `roci-agent session recover-export`.
#[derive(Parser, Debug)]
pub struct SessionRecoverExportArgs {
    /// Session root directory. Defaults to the app data session directory.
    #[arg(long, value_name = "PATH")]
    pub root: Option<PathBuf>,

    /// Session id to recover.
    #[arg(
        value_name = "ID",
        conflicts_with = "session_dir",
        required_unless_present = "session_dir"
    )]
    pub id: Option<String>,

    /// Session directory to recover.
    #[arg(long, value_name = "PATH", conflicts_with = "id")]
    pub session_dir: Option<PathBuf>,

    /// Source session id when recovering from --session-dir.
    #[arg(long, value_name = "ID", requires = "session_dir")]
    pub source_id: Option<String>,

    /// Output recovered session JSON path.
    #[arg(long, value_name = "PATH")]
    pub output: PathBuf,

    /// Print a JSON summary.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `roci-agent session recover-import`.
#[derive(Parser, Debug)]
pub struct SessionRecoverImportArgs {
    /// Session root directory. Defaults to the app data session directory.
    #[arg(long, value_name = "PATH")]
    pub root: Option<PathBuf>,

    /// Input recovered session JSON path.
    #[arg(long, value_name = "PATH")]
    pub input: PathBuf,

    /// Imported session id.
    #[arg(long, value_name = "ID")]
    pub id: String,

    /// Print a JSON summary.
    #[arg(long)]
    pub json: bool,
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
                assert!(args.candidate_models.is_empty());
                assert_eq!(args.retry_mode, ChatRetryModeArg::Bounded);
                assert_eq!(args.max_retry_attempts, 3);
                assert!(args.system.is_none());
                assert!(args.temperature.is_none());
                assert!(args.skill_path.is_empty());
                assert!(args.skill_root.is_empty());
                assert!(!args.no_skills);
                assert!(args.agent.is_none());
                assert!(!args.no_subagents);
                assert!(!args.list_agents);
                assert!(!args.no_tools);
                assert!(args.tools.is_empty());
                assert!(args.exclude_tools.is_empty());
                assert_eq!(args.max_tokens, None);
                assert!(args.context_window_override.is_none());
                assert!(args.reserve_output_tokens.is_none());
                assert!(args.max_turn_input_tokens.is_none());
                assert!(args.max_session_input_tokens.is_none());
                assert!(args.max_session_output_tokens.is_none());
                assert!(!args.no_auto_compaction);
                assert!(args.compaction_reserve_tokens.is_none());
                assert!(args.compaction_keep_recent_tokens.is_none());
                assert!(args.compaction_model.is_none());
                assert_eq!(args.approval, ChatApprovalArg::Ask);
                assert!(args.session_root.is_none());
                assert!(args.session_id.is_none());
                assert!(args.mcp_stdio.is_empty());
                assert!(args.mcp_streamable_http.is_empty());
                assert!(args.mcp_websocket.is_empty());
                assert!(args.prompt.is_none());
            }
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn parse_models_list_with_defaults() {
        let cli = Cli::try_parse_from(["roci-agent", "models", "list"]).unwrap();
        match cli.command {
            Commands::Models(models) => match models.command {
                ModelsCommands::List(args) => {
                    assert!(args.provider.is_none());
                    assert!(!args.json);
                }
                other => panic!("expected List, got {other:?}"),
            },
            other => panic!("expected Models, got {other:?}"),
        }
    }

    #[test]
    fn parse_models_list_with_provider_and_json() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "models",
            "list",
            "--provider",
            "openai",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Commands::Models(models) => match models.command {
                ModelsCommands::List(args) => {
                    assert_eq!(args.provider.as_deref(), Some("openai"));
                    assert!(args.json);
                }
                other => panic!("expected List, got {other:?}"),
            },
            other => panic!("expected Models, got {other:?}"),
        }
    }

    #[test]
    fn parse_models_switch_smoke_hidden_command() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "models",
            "switch-smoke",
            "--from",
            "openai:gpt-4o",
            "--to",
            "openai:gpt-4.1",
            "--json",
        ])
        .unwrap();

        match cli.command {
            Commands::Models(models) => match models.command {
                ModelsCommands::SwitchSmoke(args) => {
                    assert_eq!(args.from, "openai:gpt-4o");
                    assert_eq!(args.to, "openai:gpt-4.1");
                    assert!(args.json);
                }
                other => panic!("expected SwitchSmoke, got {other:?}"),
            },
            other => panic!("expected Models, got {other:?}"),
        }
    }

    #[test]
    fn parse_models_switch_chat_smoke_hidden_command() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "models",
            "switch-chat-smoke",
            "--from",
            "openai:gpt-4o",
            "--to",
            "openai:gpt-4.1",
            "--prompt",
            "Reply ok",
            "--expect",
            "ok",
            "--json",
        ])
        .unwrap();

        match cli.command {
            Commands::Models(models) => match models.command {
                ModelsCommands::SwitchChatSmoke(args) => {
                    assert_eq!(args.from, "openai:gpt-4o");
                    assert_eq!(args.to, "openai:gpt-4.1");
                    assert_eq!(args.prompt, "Reply ok");
                    assert_eq!(args.expect.as_deref(), Some("ok"));
                    assert!(args.json);
                }
                other => panic!("expected SwitchChatSmoke, got {other:?}"),
            },
            other => panic!("expected Models, got {other:?}"),
        }
    }

    #[test]
    fn parse_audio_transcribe_with_json() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "audio",
            "transcribe",
            "--input",
            "sample.wav",
            "--language",
            "en",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Commands::Audio(audio) => match audio.command {
                AudioCommands::Transcribe(args) => {
                    assert_eq!(args.input, PathBuf::from("sample.wav"));
                    assert_eq!(args.language.as_deref(), Some("en"));
                    assert_eq!(args.model, "whisper-1");
                    assert!(args.json);
                }
                other => panic!("expected Transcribe, got {other:?}"),
            },
            other => panic!("expected Audio, got {other:?}"),
        }
    }

    #[test]
    fn parse_audio_speak_with_defaults() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "audio",
            "speak",
            "--output",
            "out.mp3",
            "hello world",
        ])
        .unwrap();
        match cli.command {
            Commands::Audio(audio) => match audio.command {
                AudioCommands::Speak(args) => {
                    assert_eq!(args.output, PathBuf::from("out.mp3"));
                    assert_eq!(args.voice, "alloy");
                    assert_eq!(args.format, AudioFormatArg::Mp3);
                    assert_eq!(args.model, "tts-1");
                    assert_eq!(args.text, "hello world");
                }
                other => panic!("expected Speak, got {other:?}"),
            },
            other => panic!("expected Audio, got {other:?}"),
        }
    }

    #[test]
    fn parse_audio_speak_with_all_options() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "audio",
            "speak",
            "--output",
            "-",
            "--voice",
            "nova",
            "--format",
            "wav",
            "--speed",
            "1.25",
            "--model",
            "gpt-4o-mini-tts",
            "hello world",
        ])
        .unwrap();
        match cli.command {
            Commands::Audio(audio) => match audio.command {
                AudioCommands::Speak(args) => {
                    assert_eq!(args.output, PathBuf::from("-"));
                    assert_eq!(args.voice, "nova");
                    assert_eq!(args.format, AudioFormatArg::Wav);
                    assert_eq!(args.speed, Some(1.25));
                    assert_eq!(args.model, "gpt-4o-mini-tts");
                }
                other => panic!("expected Speak, got {other:?}"),
            },
            other => panic!("expected Audio, got {other:?}"),
        }
    }

    #[test]
    fn parse_audio_speak_rejects_speed_outside_openai_range() {
        assert!(Cli::try_parse_from([
            "roci-agent",
            "audio",
            "speak",
            "--output",
            "out.mp3",
            "--speed",
            "0.24",
            "hello world",
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "roci-agent",
            "audio",
            "speak",
            "--output",
            "out.mp3",
            "--speed",
            "4.01",
            "hello world",
        ])
        .is_err());
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
            "--approval",
            "always",
            "Hello world",
        ])
        .unwrap();
        match cli.command {
            Commands::Chat(args) => {
                assert_eq!(args.model, "anthropic:claude-4-sonnet");
                assert!(args.candidate_models.is_empty());
                assert_eq!(args.retry_mode, ChatRetryModeArg::Bounded);
                assert_eq!(args.max_retry_attempts, 3);
                assert_eq!(args.system.as_deref(), Some("You are helpful"));
                assert!((args.temperature.unwrap() - 0.7).abs() < f64::EPSILON);
                assert!(args.skill_path.is_empty());
                assert!(args.skill_root.is_empty());
                assert!(!args.no_skills);
                assert!(args.agent.is_none());
                assert!(!args.no_subagents);
                assert!(!args.list_agents);
                assert!(!args.no_tools);
                assert!(args.tools.is_empty());
                assert!(args.exclude_tools.is_empty());
                assert_eq!(args.max_tokens, Some(1024));
                assert!(args.context_window_override.is_none());
                assert!(args.reserve_output_tokens.is_none());
                assert!(args.max_turn_input_tokens.is_none());
                assert!(args.max_session_input_tokens.is_none());
                assert!(args.max_session_output_tokens.is_none());
                assert!(!args.no_auto_compaction);
                assert!(args.compaction_reserve_tokens.is_none());
                assert!(args.compaction_keep_recent_tokens.is_none());
                assert!(args.compaction_model.is_none());
                assert_eq!(args.approval, ChatApprovalArg::Always);
                assert!(args.session_root.is_none());
                assert!(args.session_id.is_none());
                assert!(args.mcp_stdio.is_empty());
                assert!(args.mcp_streamable_http.is_empty());
                assert!(args.mcp_websocket.is_empty());
                assert_eq!(args.prompt.as_deref(), Some("Hello world"));
            }
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn parse_chat_with_context_budget_and_compaction_flags() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "chat",
            "--context-window-override",
            "65536",
            "--reserve-output-tokens",
            "2048",
            "--max-turn-input-tokens",
            "4096",
            "--max-session-input-tokens",
            "262144",
            "--max-session-output-tokens",
            "16384",
            "--no-auto-compaction",
            "--compaction-reserve-tokens",
            "8192",
            "--compaction-keep-recent-tokens",
            "12000",
            "--compaction-model",
            "openai:gpt-4.1",
            "budget-check",
        ])
        .unwrap();

        match cli.command {
            Commands::Chat(args) => {
                assert_eq!(args.context_window_override, Some(65_536));
                assert_eq!(args.reserve_output_tokens, Some(2_048));
                assert_eq!(args.max_turn_input_tokens, Some(4_096));
                assert_eq!(args.max_session_input_tokens, Some(262_144));
                assert_eq!(args.max_session_output_tokens, Some(16_384));
                assert!(args.no_auto_compaction);
                assert_eq!(args.compaction_reserve_tokens, Some(8_192));
                assert_eq!(args.compaction_keep_recent_tokens, Some(12_000));
                assert_eq!(args.compaction_model.as_deref(), Some("openai:gpt-4.1"));
                assert_eq!(args.prompt.as_deref(), Some("budget-check"));
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
                assert!(args.mcp_stdio.is_empty());
                assert!(args.mcp_streamable_http.is_empty());
                assert!(args.mcp_websocket.is_empty());
            }
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn parse_chat_with_subagent_options() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "chat",
            "--agent",
            "developer",
            "--no-subagents",
            "--list-agents",
        ])
        .unwrap();
        match cli.command {
            Commands::Chat(args) => {
                assert_eq!(args.agent.as_deref(), Some("developer"));
                assert!(args.no_subagents);
                assert!(args.list_agents);
                assert!(args.prompt.is_none());
            }
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn parse_chat_with_session_options() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "chat",
            "--session-root",
            "/tmp/roci-sessions",
            "--session-id",
            "session-live",
            "prompt",
        ])
        .unwrap();
        match cli.command {
            Commands::Chat(args) => {
                assert_eq!(args.session_root, Some(PathBuf::from("/tmp/roci-sessions")));
                assert_eq!(args.session_id.as_deref(), Some("session-live"));
                assert_eq!(args.prompt.as_deref(), Some("prompt"));
            }
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn parse_chat_session_id_requires_session_root() {
        assert!(Cli::try_parse_from([
            "roci-agent",
            "chat",
            "--session-id",
            "session-live",
            "prompt"
        ])
        .is_err());
    }

    #[test]
    fn parse_session_create_with_all_options() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "session",
            "create",
            "--root",
            "/tmp/roci-sessions",
            "--id",
            "session-live",
            "--title",
            "Live session",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Commands::Session(session) => match session.command {
                SessionCommands::Create(args) => {
                    assert_eq!(args.root, Some(PathBuf::from("/tmp/roci-sessions")));
                    assert_eq!(args.id.as_deref(), Some("session-live"));
                    assert_eq!(args.title.as_deref(), Some("Live session"));
                    assert!(args.json);
                }
                other => panic!("expected Create, got {other:?}"),
            },
            other => panic!("expected Session, got {other:?}"),
        }
    }

    #[test]
    fn parse_session_list_with_root() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "session",
            "list",
            "--root",
            "/tmp/roci-sessions",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Commands::Session(session) => match session.command {
                SessionCommands::List(args) => {
                    assert_eq!(args.root, Some(PathBuf::from("/tmp/roci-sessions")));
                    assert!(args.json);
                }
                other => panic!("expected List, got {other:?}"),
            },
            other => panic!("expected Session, got {other:?}"),
        }
    }

    #[test]
    fn parse_session_delete_with_id() {
        let cli = Cli::try_parse_from(["roci-agent", "session", "delete", "session-live"]).unwrap();
        match cli.command {
            Commands::Session(session) => match session.command {
                SessionCommands::Delete(args) => {
                    assert!(args.root.is_none());
                    assert_eq!(args.id, "session-live");
                }
                other => panic!("expected Delete, got {other:?}"),
            },
            other => panic!("expected Session, got {other:?}"),
        }
    }

    #[test]
    fn parse_session_export_with_output_and_json() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "session",
            "export",
            "session-live",
            "--output",
            "/tmp/session.json",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Commands::Session(session) => match session.command {
                SessionCommands::Export(args) => {
                    assert_eq!(args.id, "session-live");
                    assert_eq!(args.output, PathBuf::from("/tmp/session.json"));
                    assert!(args.json);
                }
                other => panic!("expected Export, got {other:?}"),
            },
            other => panic!("expected Session, got {other:?}"),
        }
    }

    #[test]
    fn parse_session_import_with_input_and_id() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "session",
            "import",
            "--input",
            "/tmp/session.json",
            "--id",
            "imported-session",
        ])
        .unwrap();
        match cli.command {
            Commands::Session(session) => match session.command {
                SessionCommands::Import(args) => {
                    assert_eq!(args.input, PathBuf::from("/tmp/session.json"));
                    assert_eq!(args.id.as_deref(), Some("imported-session"));
                    assert!(!args.json);
                }
                other => panic!("expected Import, got {other:?}"),
            },
            other => panic!("expected Session, got {other:?}"),
        }
    }

    #[test]
    fn parse_chat_tool_visibility_flags() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "chat",
            "--no-tools",
            "--tool",
            "read_file",
            "--tool",
            "grep",
            "--exclude-tool",
            "shell",
            "prompt",
        ])
        .unwrap();
        match cli.command {
            Commands::Chat(args) => {
                assert!(args.no_tools);
                assert_eq!(args.tools, vec!["read_file", "grep"]);
                assert_eq!(args.exclude_tools, vec!["shell"]);
                assert_eq!(args.prompt.as_deref(), Some("prompt"));
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
    fn parse_chat_with_repeatable_attachments() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "chat",
            "--attach",
            "notes.md",
            "--attach",
            "diagram.png",
            "prompt text",
        ])
        .unwrap();
        match cli.command {
            Commands::Chat(args) => {
                assert_eq!(
                    args.attachments,
                    vec![PathBuf::from("notes.md"), PathBuf::from("diagram.png")]
                );
                assert_eq!(args.prompt.as_deref(), Some("prompt text"));
            }
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn parse_chat_attachment_requires_a_path_value() {
        assert!(Cli::try_parse_from(["roci-agent", "chat", "--attach"]).is_err());
    }

    #[test]
    fn parse_chat_with_candidate_models_and_retry_controls() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "chat",
            "--model",
            "openai:gpt-4o",
            "--candidate-model",
            "anthropic:claude-sonnet-4-5",
            "--candidate-model",
            "google:gemini-2.5-pro",
            "--retry-mode",
            "bounded",
            "--max-retry-attempts",
            "1",
            "Hi",
        ])
        .unwrap();

        match cli.command {
            Commands::Chat(args) => {
                assert_eq!(args.model, "openai:gpt-4o");
                assert_eq!(
                    args.candidate_models,
                    vec![
                        "anthropic:claude-sonnet-4-5".to_string(),
                        "google:gemini-2.5-pro".to_string()
                    ]
                );
                assert_eq!(args.retry_mode, ChatRetryModeArg::Bounded);
                assert_eq!(args.max_retry_attempts, 1);
                assert_eq!(args.prompt.as_deref(), Some("Hi"));
            }
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn parse_chat_rejects_zero_retry_attempts() {
        let err = Cli::try_parse_from(["roci-agent", "chat", "--max-retry-attempts", "0", "Hi"])
            .expect_err("zero retry attempts should fail parsing");

        assert!(err.to_string().contains("max-retry-attempts must be >= 1"));
    }

    #[test]
    fn parse_chat_with_persistent_retry_mode() {
        let cli = Cli::try_parse_from(["roci-agent", "chat", "--retry-mode", "persistent", "Hi"])
            .unwrap();

        match cli.command {
            Commands::Chat(args) => {
                assert_eq!(args.retry_mode, ChatRetryModeArg::Persistent);
                assert_eq!(args.prompt.as_deref(), Some("Hi"));
            }
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn parse_chat_with_repeatable_mcp_specs() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "chat",
            "--mcp-stdio",
            "id=local,command=npx,arg=-y,arg=@modelcontextprotocol/server-filesystem",
            "--mcp-stdio",
            "command=uvx,arg=echo",
            "--mcp-streamable-http",
            "id=docs,url=http://localhost:3000/mcp,header=x-env:dev",
            "--mcp-streamable-http",
            "url=https://example.com/mcp,auth_token=secret",
            "--mcp-websocket",
            "id=ws,url=ws://localhost:3001/mcp,header=x-env:test",
            "Hi",
        ])
        .unwrap();

        match cli.command {
            Commands::Chat(args) => {
                assert_eq!(
                    args.mcp_stdio,
                    vec![
                        "id=local,command=npx,arg=-y,arg=@modelcontextprotocol/server-filesystem"
                            .to_string(),
                        "command=uvx,arg=echo".to_string()
                    ]
                );
                assert_eq!(
                    args.mcp_streamable_http,
                    vec![
                        "id=docs,url=http://localhost:3000/mcp,header=x-env:dev".to_string(),
                        "url=https://example.com/mcp,auth_token=secret".to_string()
                    ]
                );
                assert_eq!(
                    args.mcp_websocket,
                    vec!["id=ws,url=ws://localhost:3001/mcp,header=x-env:test".to_string()]
                );
                assert_eq!(args.prompt.as_deref(), Some("Hi"));
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

    #[test]
    fn parse_tool_contracts_smoke_defaults_to_all_case() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "tool-contracts-smoke",
            "--model",
            "openai-compatible:gpt-4o-mini",
        ])
        .unwrap();

        match cli.command {
            Commands::ToolContractsSmoke(args) => {
                assert_eq!(args.model, "openai-compatible:gpt-4o-mini");
                assert_eq!(args.case, ToolContractsSmokeCaseArg::All);
                assert!(args.endpoint.is_none());
                assert!(args.api_key.is_none());
            }
            other => panic!("expected ToolContractsSmoke, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_contracts_smoke_accepts_endpoint_api_key_and_case() {
        let cli = Cli::try_parse_from([
            "roci-agent",
            "tool-contracts-smoke",
            "--model",
            "openai-compatible:gpt-4o-mini",
            "--endpoint",
            "http://framed:4001/v1",
            "--api-key",
            "sk-local-dummy",
            "--case",
            "alias",
        ])
        .unwrap();

        match cli.command {
            Commands::ToolContractsSmoke(args) => {
                assert_eq!(args.model, "openai-compatible:gpt-4o-mini");
                assert_eq!(args.endpoint.as_deref(), Some("http://framed:4001/v1"));
                assert_eq!(args.api_key.as_deref(), Some("sk-local-dummy"));
                assert_eq!(args.case, ToolContractsSmokeCaseArg::Alias);
            }
            other => panic!("expected ToolContractsSmoke, got {other:?}"),
        }
    }
}
