//! Roci CLI binary entry point.

use std::sync::Arc;

use clap::Parser;
use roci::agent_loop::{
    ApprovalPolicy, LoopRunner, RunEventPayload, RunLifecycle, RunRequest, RunStatus, Runner,
};
use roci::cli::{AuthCommands, ChatArgs, Cli, Commands};
use roci::config::RociConfig;
use roci::types::ModelMessage;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Auth(auth_args) => match auth_args.command {
            AuthCommands::Login(args) => roci::cli::auth::handle_login(&args.provider).await,
            AuthCommands::Status => roci::cli::auth::handle_status().await,
            AuthCommands::Logout(args) => roci::cli::auth::handle_logout(&args.provider).await,
        },
        Commands::Chat(chat_args) => handle_chat(chat_args).await,
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

async fn handle_chat(args: ChatArgs) -> Result<(), Box<dyn std::error::Error>> {
    let prompt = match args.prompt {
        Some(p) => p,
        None => {
            eprintln!("Usage: roci chat \"your prompt here\"");
            std::process::exit(1);
        }
    };

    let model: roci::models::LanguageModel = args.model.parse().map_err(|_| {
        format!(
            "Invalid model format: '{}'. Use provider:model (e.g. openai:gpt-4o)",
            args.model
        )
    })?;

    let config = RociConfig::from_env();
    let runner = LoopRunner::new(config);

    let mut messages = Vec::new();
    if let Some(system) = args.system {
        messages.push(ModelMessage::system(system));
    }
    messages.push(ModelMessage::user(prompt));

    let mut settings = roci::types::GenerationSettings::default();
    if let Some(t) = args.temperature {
        settings.temperature = Some(t);
    }
    if let Some(max) = args.max_tokens {
        settings.max_tokens = Some(max);
    }

    // Stream events to terminal
    let sink = Arc::new(|event: roci::agent_loop::RunEvent| {
        use std::io::Write;
        match &event.payload {
            RunEventPayload::AssistantDelta { text } => {
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
            RunEventPayload::ToolCallStarted { call } => {
                eprintln!("\n⚡ {} ({})", call.name, call.id);
            }
            RunEventPayload::ToolResult { result } => {
                let output = result.result.to_string();
                let truncated = if output.len() > 200 {
                    // Find a valid UTF-8 char boundary at or before 200
                    let mut end = 200;
                    while end > 0 && !output.is_char_boundary(end) {
                        end -= 1;
                    }
                    format!("{}...", &output[..end])
                } else {
                    output
                };
                if result.is_error {
                    eprintln!("  ❌ {truncated}");
                } else {
                    eprintln!("  ✅ {truncated}");
                }
            }
            RunEventPayload::Lifecycle {
                state: RunLifecycle::Failed { error },
            } => {
                eprintln!("\n❌ {error}");
            }
            _ => {}
        }
    });

    // Register built-in tools
    let tools = roci::tools::builtin::all_tools();

    let mut request = RunRequest::new(model, messages);
    request.settings = settings;
    request.event_sink = Some(sink);
    request.tools = tools;
    request.approval_policy = ApprovalPolicy::Always;

    let handle = runner.start(request).await?;
    let result = handle.wait().await;

    println!(); // newline after streaming

    if result.status == RunStatus::Failed {
        if let Some(err) = result.error {
            return Err(err.into());
        }
    }

    Ok(())
}
