use std::collections::HashMap;
use std::sync::Arc;

use roci::agent::{AgentConfig, AgentRuntime, QueueDrainMode, UserInputCoordinator};
use roci::agent_loop::{AgentEvent, PreToolUseHookResult, RunStatus};
use roci::config::RociConfig;
use roci::mcp::{merge_mcp_instructions, MCPInstructionMergePolicy};
use roci::resource::SkillResourceOptions;
use roci::skills::merge_system_prompt_with_skills;
use roci::types::{ContentPart, StreamEventType};

use crate::cli::ChatArgs;

mod mcp;
mod resource_prompt;
mod user_input;

use mcp::build_mcp_runtime_wiring;
use resource_prompt::{
    build_resource_system_prompt, expand_chat_prompt, print_resource_diagnostics, truncate_preview,
};
use user_input::PromptHost;

pub async fn handle_chat(args: ChatArgs) -> Result<(), Box<dyn std::error::Error>> {
    let ChatArgs {
        model: model_arg,
        system,
        temperature,
        skill_path,
        skill_root,
        no_skills,
        max_tokens,
        mcp_stdio,
        mcp_sse,
        prompt,
    } = args;

    let prompt = match prompt {
        Some(p) => p,
        None => {
            eprintln!("Usage: roci-agent chat \"your prompt here\"");
            std::process::exit(1);
        }
    };

    let model: roci::models::LanguageModel = model_arg.parse().map_err(|_| {
        format!(
            "Invalid model format: '{}'. Use provider:model (e.g. openai:gpt-4o)",
            model_arg
        )
    })?;

    let config = RociConfig::from_env();
    let registry = Arc::new(roci::default_registry());
    let cwd = std::env::current_dir()?;

    let skill_options = SkillResourceOptions {
        enabled: !no_skills,
        explicit_paths: skill_path,
        extra_roots: skill_root,
    };

    let resources = roci::resource::ResourceLoader::new()
        .with_skill_options(skill_options)
        .load(&cwd)?;
    print_resource_diagnostics(&resources);

    let prompt = expand_chat_prompt(&prompt, &resources);
    let resource_system_prompt = build_resource_system_prompt(system, &resources);
    let skill_system_prompt =
        merge_system_prompt_with_skills(resource_system_prompt, &resources.skills.skills);
    let mcp_runtime = build_mcp_runtime_wiring(&mcp_stdio, &mcp_sse).await?;
    let system_prompt = merge_mcp_instructions(
        skill_system_prompt.as_deref(),
        &mcp_runtime.instructions,
        MCPInstructionMergePolicy::AppendBlock,
    );

    let mut settings = roci::types::GenerationSettings::default();
    if let Some(t) = temperature {
        settings.temperature = Some(t);
    }
    if let Some(max) = max_tokens {
        settings.max_tokens = Some(max);
    }

    let coordinator = Arc::new(UserInputCoordinator::new());
    let prompt_host = PromptHost::spawn(coordinator.clone());
    let tools = roci_tools::builtin::all_tools();
    let agent = AgentRuntime::new(
        registry,
        config,
        AgentConfig {
            model,
            system_prompt,
            tools,
            dynamic_tool_providers: mcp_runtime.dynamic_tool_providers,
            settings,
            transform_context: None,
            convert_to_llm: None,
            before_agent_start: None,
            event_sink: Some(prompt_host.build_agent_sink()),
            session_id: None,
            steering_mode: QueueDrainMode::All,
            follow_up_mode: QueueDrainMode::All,
            transport: None,
            max_retry_delay_ms: None,
            retry_backoff: Default::default(),
            api_key_override: None,
            provider_headers: Default::default(),
            provider_metadata: HashMap::new(),
            provider_payload_callback: None,
            get_api_key: None,
            compaction: Default::default(),
            session_before_compact: None,
            session_before_tree: None,
            pre_tool_use: Some(Arc::new(|call, _cancel| {
                demo_pre_tool_use_hook(&call.name, &call.id);
                Box::pin(async { Ok(PreToolUseHookResult::Continue) })
            })),
            post_tool_use: Some(Arc::new(|call, result| {
                demo_post_tool_use_hook(&call.name, &call.id);
                Box::pin(async move { Ok(result) })
            })),
            user_input_timeout_ms: None,
            context_budget: None,
            chat: Default::default(),
            user_input_coordinator: Some(coordinator.clone()),
        },
    );

    let result = agent.prompt(prompt).await;
    prompt_host.shutdown();
    let result = result?;
    println!();

    if result.status == RunStatus::Failed {
        if let Some(err) = result.error {
            return Err(err.into());
        }
    }

    Ok(())
}

fn render_agent_event(event: AgentEvent) {
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
                if let ContentPart::Text { text } = part {
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
}

fn demo_pre_tool_use_hook(tool_name: &str, tool_call_id: &str) {
    eprintln!("[hook] preToolUse called (tool={tool_name}, id={tool_call_id})");
}

fn demo_post_tool_use_hook(tool_name: &str, tool_call_id: &str) {
    eprintln!("[hook] postToolUse called (tool={tool_name}, id={tool_call_id})");
}

#[cfg(test)]
mod tests {
    #[test]
    fn copilot_provider_available_in_default_registry() {
        let registry = roci::default_registry();
        assert!(
            registry.has_provider("github-copilot"),
            "expected github-copilot provider to be registered in default roci-cli builds"
        );
    }
}
