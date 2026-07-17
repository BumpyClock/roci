use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use roci::agent::{AgentConfig, AgentRuntime, HumanInteractionCoordinator, QueueDrainMode};
use roci::agent_loop::{ApprovalPolicy, PreToolUseHookResult, RetryMode, RunStatus};
use roci::attachments::{Attachment, PromptInput};
use roci::config::RociConfig;
use roci::context::ContextBudget;
use roci::mcp::{merge_mcp_instructions, MCPInstructionMergePolicy};
use roci::resource::CompactionSettings;
use roci::resource::SkillResourceOptions;
use roci::session::{
    CreateSessionOptions, LocalSessionStore, SessionConfig, SessionId, SessionModelPreferences,
    SessionResumeState,
};
use roci::skills::merge_system_prompt_with_skills;
use roci::tools::ToolVisibilityPolicy;

use crate::cli::{ChatApprovalArg, ChatArgs, ChatRetryModeArg};

mod mcp;
mod resource_prompt;
mod runtime_events;
mod subagents;
mod user_input;

use mcp::build_mcp_runtime_wiring;
use resource_prompt::{
    build_resource_system_prompt, expand_chat_prompt, print_resource_diagnostics,
};
use runtime_events::RuntimeEventRenderer;
use subagents::{load_cli_subagent_profiles, print_agent_profiles, select_session_agent_profile};

pub async fn handle_chat(args: ChatArgs) -> Result<(), Box<dyn std::error::Error>> {
    let ChatArgs {
        model: model_arg,
        candidate_models,
        retry_mode,
        max_retry_attempts,
        system,
        temperature,
        skill_path,
        skill_root,
        no_skills,
        agent,
        no_subagents,
        list_agents,
        no_tools,
        tools: allowed_tools,
        exclude_tools,
        context_window_override,
        reserve_output_tokens,
        max_turn_input_tokens,
        max_session_input_tokens,
        max_session_output_tokens,
        no_auto_compaction,
        compaction_reserve_tokens,
        compaction_keep_recent_tokens,
        compaction_model,
        max_tokens,
        approval,
        session_root,
        session_id,
        attachments,
        mcp_stdio,
        mcp_streamable_http,
        mcp_websocket,
        prompt,
    } = args;

    let cwd = std::env::current_dir()?;
    if list_agents {
        let subagent_profiles = load_cli_subagent_profiles(&cwd, agent)?;
        let mut stdout = std::io::stdout();
        print_agent_profiles(&subagent_profiles.registry, &mut stdout)?;
        return Ok(());
    }
    let config = RociConfig::from_env();
    let registry = Arc::new(roci::default_registry());

    let model: roci::models::LanguageModel = model_arg.parse().map_err(|_| {
        format!(
            "Invalid model format: '{}'. Use provider:model (e.g. openai:gpt-4o)",
            model_arg
        )
    })?;
    let mut candidates = vec![model];
    for candidate in candidate_models {
        let parsed = candidate.parse().map_err(|_| {
            format!(
                "Invalid candidate model format: '{}'. Use provider:model (e.g. openai:gpt-4o)",
                candidate
            )
        })?;
        candidates.push(parsed);
    }
    let prompt = match prompt {
        Some(p) => p,
        None => {
            eprintln!("No prompt provided.");
            eprintln!("Usage: roci-agent chat \"your prompt here\"");
            eprintln!("Or pipe stdin: echo \"your prompt here\" | roci-agent chat");
            std::process::exit(1);
        }
    };
    let retry_mode = match retry_mode {
        ChatRetryModeArg::Bounded => Some(RetryMode::Bounded {
            max_attempts: max_retry_attempts,
        }),
        ChatRetryModeArg::Persistent => Some(RetryMode::Persistent),
    };

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
    let prompt_input = build_prompt_input(prompt, &attachments);
    let resource_system_prompt = build_resource_system_prompt(system, &resources);
    let skill_system_prompt =
        merge_system_prompt_with_skills(resource_system_prompt, &resources.skills.skills);
    let mcp_runtime =
        build_mcp_runtime_wiring(&mcp_stdio, &mcp_streamable_http, &mcp_websocket).await?;
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
    let context_budget = build_context_budget(
        context_window_override,
        reserve_output_tokens,
        max_turn_input_tokens,
        max_session_input_tokens,
        max_session_output_tokens,
    );
    let compaction = {
        let default = CompactionSettings::default();
        CompactionSettings {
            enabled: !no_auto_compaction,
            reserve_tokens: compaction_reserve_tokens.unwrap_or(default.reserve_tokens),
            keep_recent_tokens: compaction_keep_recent_tokens.unwrap_or(default.keep_recent_tokens),
            model: compaction_model,
        }
    };

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
    let session = session_root
        .map(|root| {
            let id = match session_id {
                Some(id) => SessionId::parse(id),
                None => Ok(SessionId::new_v4()),
            }?;
            Ok::<_, roci::session::SessionError>(SessionConfig::new(id, root))
        })
        .transpose()?;
    let mut session_state = None;
    let persisted_agent_profile = if let Some(session_config) = session.as_ref() {
        if session_config.conventions().metadata_file().is_file() {
            let store = LocalSessionStore::new(session_config.root.clone());
            let state = store.open(session_config.id.clone()).await?;
            let persisted = state.metadata.agent_profile.clone();
            session_state = Some(state);
            persisted
        } else {
            None
        }
    } else {
        None
    };
    let selected_agent_profile =
        select_session_agent_profile(agent.as_deref(), persisted_agent_profile.as_deref());
    let subagent_profiles = load_cli_subagent_profiles(&cwd, selected_agent_profile.clone())?;
    if let Some(state) = session_state.as_mut() {
        let store = LocalSessionStore::new(state.session_config.root.clone());
        persist_explicit_agent_profile(&store, state, agent.as_deref())?;
    }
    if let Some(session_config) = session.as_ref() {
        if session_state.is_none() {
            let store = LocalSessionStore::new(session_config.root.clone());
            session_state = Some(
                store
                    .create(CreateSessionOptions {
                        id: Some(session_config.id.clone()),
                        title: None,
                        host_cwd: Some(cwd.clone()),
                        import_source: None,
                        default_thread_id: None,
                        model_preferences: SessionModelPreferences {
                            agent_profile: selected_agent_profile,
                            ..SessionModelPreferences::default()
                        },
                    })
                    .await?,
            );
        }
    }
    let tools = roci_tools::builtin::tool_catalog().resolve(&tool_visibility_policy);
    let agent_config = AgentConfig {
        candidates,
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
        session,
        workspace_root: Some(cwd.clone()),
        sandbox_provider: None,
        steering_mode: QueueDrainMode::All,
        follow_up_mode: QueueDrainMode::All,
        transport: None,
        max_retry_delay_ms: None,
        retry_backoff: Default::default(),
        retry_mode,
        model_health: Default::default(),
        api_key_override: None,
        provider_headers: Default::default(),
        provider_metadata: HashMap::new(),
        provider_payload_callback: None,
        get_api_key: None,
        compaction,
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
        context_budget,
        chat: Default::default(),
        subagents: subagent_profiles.into_config(!no_subagents),
        human_interaction_coordinator: Some(coordinator.clone()),
    };
    let agent = if let Some(state) = session_state {
        Arc::new(AgentRuntime::resume_session(registry, config, agent_config, state).await?)
    } else {
        Arc::new(AgentRuntime::try_new(registry, config, agent_config)?)
    };

    let subscription = agent.subscribe(None).await;
    renderer.subscribe(subscription, agent.clone());

    let result = agent.prompt(prompt_input).await;
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

fn persist_explicit_agent_profile(
    store: &LocalSessionStore,
    state: &mut SessionResumeState,
    explicit_profile: Option<&str>,
) -> Result<(), roci::session::SessionError> {
    let Some(agent_profile) = explicit_profile else {
        return Ok(());
    };
    let id = state.metadata.id.clone();
    state.metadata = store.update_agent_profile(&id, Some(agent_profile.to_string()))?;
    Ok(())
}

fn build_prompt_input(prompt: String, attachment_paths: &[PathBuf]) -> PromptInput {
    PromptInput::new(prompt)
        .with_attachments(attachment_paths.iter().cloned().map(Attachment::file))
}

fn build_context_budget(
    context_window_override: Option<usize>,
    reserve_output_tokens: Option<usize>,
    max_turn_input_tokens: Option<usize>,
    max_session_input_tokens: Option<usize>,
    max_session_output_tokens: Option<usize>,
) -> Option<ContextBudget> {
    if context_window_override.is_none()
        && reserve_output_tokens.is_none()
        && max_turn_input_tokens.is_none()
        && max_session_input_tokens.is_none()
        && max_session_output_tokens.is_none()
    {
        return None;
    }

    let default = ContextBudget::default();
    Some(ContextBudget {
        context_window_override,
        reserve_output_tokens: reserve_output_tokens.unwrap_or(default.reserve_output_tokens),
        max_turn_input_tokens,
        max_session_input_tokens,
        max_session_output_tokens,
    })
}

fn demo_pre_tool_use_hook(tool_name: &str, tool_call_id: &str) {
    eprintln!("[hook] preToolUse called (tool={tool_name}, id={tool_call_id})");
}

fn demo_post_tool_use_hook(tool_name: &str, tool_call_id: &str) {
    eprintln!("[hook] postToolUse called (tool={tool_name}, id={tool_call_id})");
}

fn approval_policy_from_arg(arg: ChatApprovalArg) -> ApprovalPolicy {
    match arg {
        ChatApprovalArg::Ask => ApprovalPolicy::ask(),
        ChatApprovalArg::Always => ApprovalPolicy::always(),
        ChatApprovalArg::Never => ApprovalPolicy::never(),
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
    use std::path::PathBuf;

    use roci::agent_loop::ApprovalAction;
    use roci::attachments::Attachment;
    use roci::models::LanguageModel;
    use roci::session::{
        CreateSessionOptions, LocalSessionStore, SessionId, SessionModelPreferences,
    };
    use roci::types::ReasoningEffort;
    use tempfile::tempdir;

    use super::{
        approval_policy_from_arg, build_context_budget, build_prompt_input,
        persist_explicit_agent_profile,
    };
    use crate::cli::ChatApprovalArg;

    #[test]
    fn copilot_provider_available_in_default_registry() {
        let registry = roci::default_registry();
        assert!(
            registry.has_provider("github-copilot"),
            "expected github-copilot provider to be registered in default roci-cli builds"
        );
    }

    #[test]
    fn build_prompt_input_preserves_paths_and_count() {
        let prompt = "Describe this report";
        let attachments = vec![
            PathBuf::from("/tmp/notes.txt"),
            PathBuf::from("/tmp/diagram.png"),
        ];

        let input = build_prompt_input(prompt.to_string(), &attachments);

        assert_eq!(input.text, prompt);
        assert_eq!(input.attachments.len(), attachments.len());
        for (expected_path, attachment) in attachments.iter().zip(input.attachments.iter()) {
            match attachment {
                Attachment::File(file) => assert_eq!(file.path, *expected_path),
                _ => panic!("expected file attachment"),
            }
        }
    }

    #[test]
    fn approval_arg_maps_to_policy_presets() {
        assert_eq!(
            approval_policy_from_arg(ChatApprovalArg::Ask).default_action,
            ApprovalAction::Ask
        );
        assert_eq!(
            approval_policy_from_arg(ChatApprovalArg::Always).default_action,
            ApprovalAction::Allow
        );
        assert_eq!(
            approval_policy_from_arg(ChatApprovalArg::Never).default_action,
            ApprovalAction::Deny
        );
    }

    #[test]
    fn build_context_budget_defaults_when_fields_omitted() {
        assert!(build_context_budget(None, None, None, None, None).is_none());
    }

    #[test]
    fn build_context_budget_uses_core_defaults_for_unspecified_fields() {
        let budget =
            build_context_budget(Some(65_536), Some(2_048), None, Some(500_000), None).unwrap();
        assert_eq!(budget.context_window_override, Some(65_536));
        assert_eq!(budget.reserve_output_tokens, 2_048);
        assert!(budget.max_turn_input_tokens.is_none());
        assert_eq!(budget.max_session_input_tokens, Some(500_000));
        assert!(budget.max_session_output_tokens.is_none());
    }

    #[tokio::test]
    async fn explicit_agent_override_persists_without_changing_model_preferences() {
        let root = tempdir().unwrap();
        let store = LocalSessionStore::new(root.path());
        let id = SessionId::parse("agent-override").unwrap();
        let preferences = SessionModelPreferences {
            selected_model: Some(LanguageModel::Known {
                provider_key: "openai".into(),
                model_id: "gpt-5".into(),
            }),
            reasoning_effort: Some(ReasoningEffort::High),
            agent_profile: Some("builtin:developer".into()),
        };
        let mut state = store
            .create(CreateSessionOptions {
                id: Some(id.clone()),
                model_preferences: preferences.clone(),
                ..Default::default()
            })
            .await
            .unwrap();

        store
            .update_model_preferences(
                &id,
                SessionModelPreferences {
                    selected_model: Some(LanguageModel::Known {
                        provider_key: "google".into(),
                        model_id: "gemini-2.5-pro".into(),
                    }),
                    reasoning_effort: Some(ReasoningEffort::Medium),
                    agent_profile: Some("builtin:planner".into()),
                },
            )
            .unwrap();
        persist_explicit_agent_profile(&store, &mut state, Some("builtin:developer")).unwrap();
        drop(state);

        let reopened = store.open(id).await.unwrap();
        assert_eq!(
            reopened.metadata.selected_model,
            Some(LanguageModel::Known {
                provider_key: "google".into(),
                model_id: "gemini-2.5-pro".into(),
            })
        );
        assert_eq!(
            reopened.metadata.reasoning_effort,
            Some(ReasoningEffort::Medium)
        );
        assert_eq!(
            reopened.metadata.agent_profile.as_deref(),
            Some("builtin:developer")
        );
    }
}
