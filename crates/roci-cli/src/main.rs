//! Roci CLI binary entry point.

mod cli;
mod errors;

use std::sync::Arc;

use clap::Parser;
use roci::agent_loop::{
    AgentEvent, ApprovalPolicy, LoopRunner, RunEventPayload, RunLifecycle, RunRequest, RunStatus,
    Runner,
};
use roci::config::RociConfig;
use roci::types::{ModelMessage, StreamEventType};

use cli::{AuthCommands, ChatArgs, Cli, Commands};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Auth(auth_args) => match auth_args.command {
            AuthCommands::Login(args) => cli::auth::handle_login(&args.provider).await,
            AuthCommands::Status => cli::auth::handle_status().await,
            AuthCommands::Logout(args) => cli::auth::handle_logout(&args.provider).await,
        },
        Commands::Chat(chat_args) => handle_chat(chat_args).await,
    };

    if let Err(e) = result {
        // Try to downcast to RociError for actionable help text
        let message = if let Some(roci_err) = e.downcast_ref::<roci::error::RociError>() {
            errors::format_error_help(roci_err)
        } else {
            format!("{e}")
        };
        eprintln!("Error: {message}");
        std::process::exit(1);
    }
}

async fn handle_chat(args: ChatArgs) -> Result<(), Box<dyn std::error::Error>> {
    let prompt = match args.prompt {
        Some(p) => p,
        None => {
            eprintln!("Usage: roci-agent chat \"your prompt here\"");
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
    let registry = Arc::new(roci::default_registry());
    let runner = LoopRunner::with_registry(config, registry);

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

    // Stream run-level failures to terminal
    let sink = Arc::new(|event: roci::agent_loop::RunEvent| {
        match &event.payload {
            RunEventPayload::Lifecycle {
                state: RunLifecycle::Failed { error },
            } => {
                eprintln!("\n❌ {error}");
            }
            _ => {}
        }
    });

    // Stream richer agent events to terminal
    let agent_sink = Arc::new(|event: AgentEvent| {
        use std::io::Write;
        match event {
            AgentEvent::MessageUpdate {
                assistant_message_event,
                ..
            } => match assistant_message_event.event_type {
                StreamEventType::TextDelta => {
                    if !assistant_message_event.text.is_empty() {
                        print!("{}", assistant_message_event.text);
                        let _ = std::io::stdout().flush();
                    }
                }
                StreamEventType::Reasoning => {
                    if let Some(reasoning) = assistant_message_event.reasoning {
                        if !reasoning.is_empty() {
                            eprintln!("\n💭 {}", truncate_preview(&reasoning, 120));
                        }
                    }
                }
                _ => {}
            },
            AgentEvent::ToolExecutionStart {
                tool_name,
                tool_call_id,
                ..
            } => {
                eprintln!("\n⚡ {tool_name} ({tool_call_id})");
            }
            AgentEvent::ToolExecutionUpdate {
                tool_name,
                partial_result,
                ..
            } => {
                let preview = if let Some(text) = partial_result.content.iter().find_map(|part| {
                    if let roci::types::ContentPart::Text { text } = part {
                        Some(text.as_str())
                    } else {
                        None
                    }
                }) {
                    truncate_preview(text, 80)
                } else {
                    truncate_preview(&partial_result.details.to_string(), 80)
                };
                eprintln!("  … {tool_name}: {preview}");
            }
            AgentEvent::ToolExecutionEnd { result, .. } => {
                let preview = truncate_preview(&result.result.to_string(), 200);
                if result.is_error {
                    eprintln!("  ❌ {preview}");
                } else {
                    eprintln!("  ✅ {preview}");
                }
            }
            _ => {}
        }
    });

    // Register built-in tools
    let tools = roci_tools::builtin::all_tools();

    let mut request = RunRequest::new(model, messages);
    request.settings = settings;
    request.event_sink = Some(sink);
    request.agent_event_sink = Some(agent_sink);
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

fn truncate_preview(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let end = value
        .char_indices()
        .nth(max_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(value.len());
    format!("{}...", &value[..end])
}
