use std::collections::HashMap;
use std::sync::Arc;

use roci::agent::{AgentConfig, AgentRuntime, HumanInteractionCoordinator, QueueDrainMode};
use roci::agent_loop::{ApprovalPolicy, PreToolUseHookResult, RunStatus};
use roci::config::RociConfig;
use roci::mcp::{merge_mcp_instructions, MCPInstructionMergePolicy};
use roci::resource::SkillResourceOptions;
use roci::skills::merge_system_prompt_with_skills;
use roci::tools::ToolVisibilityPolicy;

use crate::cli::{ChatApprovalArg, ChatArgs};

mod mcp;
mod resource_prompt;
mod runtime_events;
mod user_input;

use mcp::build_mcp_runtime_wiring;
use resource_prompt::{
    build_resource_system_prompt, expand_chat_prompt, print_resource_diagnostics,
};
use runtime_events::RuntimeEventRenderer;

pub async fn handle_chat(args: ChatArgs) -> Result<(), Box<dyn std::error::Error>> {
    let ChatArgs {
        model: model_arg,
        system,
        temperature,
        skill_path,
        skill_root,
        no_skills,
        no_tools,
        tools: allowed_tools,
        exclude_tools,
        max_tokens,
        approval,
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

    let coordinator = Arc::new(HumanInteractionCoordinator::new());
    let mut renderer = RuntimeEventRenderer::spawn(coordinator.clone());
    let approval_policy = approval_policy_from_arg(approval);
    let approval_handler =
        (approval == ChatApprovalArg::Ask).then(|| renderer.build_approval_handler());
    let tool_visibility_policy = tool_visibility_policy_from_args(
        no_tools,
        allowed_tools.iter().map(String::as_str),
        exclude_tools.iter().map(String::as_str),
    );
    let tools = roci_tools::builtin::tool_catalog().resolve(&tool_visibility_policy);
    let agent = Arc::new(AgentRuntime::new(
        registry,
        config,
        AgentConfig {
            model,
            system_prompt,
            tools,
            tool_visibility_policy,
            dynamic_tool_providers: mcp_runtime.dynamic_tool_providers,
            settings,
            transform_context: None,
            convert_to_llm: None,
            before_agent_start: None,
            event_sink: Some(renderer.build_agent_sink()),
            approval_policy,
            approval_handler,
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
            human_interaction_coordinator: Some(coordinator.clone()),
        },
    ));

    let subscription = agent.subscribe(None).await;
    renderer.subscribe(subscription, agent.clone());

    let result = agent.prompt(prompt).await;
    renderer.finish().await;
    let result = result?;
    println!();

    if result.status == RunStatus::Failed {
        if let Some(err) = result.error {
            return Err(err.into());
        }
    }

    Ok(())
}

fn demo_pre_tool_use_hook(tool_name: &str, tool_call_id: &str) {
    eprintln!("[hook] preToolUse called (tool={tool_name}, id={tool_call_id})");
}

fn demo_post_tool_use_hook(tool_name: &str, tool_call_id: &str) {
    eprintln!("[hook] postToolUse called (tool={tool_name}, id={tool_call_id})");
}

fn approval_policy_from_arg(arg: ChatApprovalArg) -> ApprovalPolicy {
    match arg {
        ChatApprovalArg::Ask => ApprovalPolicy::Ask,
        ChatApprovalArg::Always => ApprovalPolicy::Always,
        ChatApprovalArg::Never => ApprovalPolicy::Never,
    }
}

fn tool_visibility_policy_from_args<'a>(
    no_tools: bool,
    allowed_tools: impl IntoIterator<Item = &'a str>,
    excluded_tools: impl IntoIterator<Item = &'a str>,
) -> ToolVisibilityPolicy {
    let mut policy = ToolVisibilityPolicy::default();
    policy.set_no_tools(no_tools);
    policy.extend_allow(allowed_tools);
    policy.extend_exclude(excluded_tools);
    policy
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
