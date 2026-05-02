//! Internal launcher trait and in-process implementation for child runtimes.

use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;

#[cfg(feature = "agent")]
use crate::agent::runtime::HumanInteractionCoordinator;
use crate::agent::runtime::{AgentConfig, AgentRuntime};
use crate::agent_loop::runner::AgentEventSink;
use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider::ProviderRegistry;
use crate::tools::tool::Tool;
use crate::types::{ModelMessage, ReasoningEffort};

use super::types::{SubagentId, ToolPolicy};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Launched child payload returned by a [`SubagentLauncher`].
pub(super) struct LaunchedChild {
    pub runtime: AgentRuntime,
}

/// Abstraction for creating child [`AgentRuntime`] instances.
///
/// The trait exists so that tests can inject a mock launcher without needing
/// real provider credentials.
///
/// `initial_messages` is the fully-composed message list from
/// [`build_child_initial_messages`](super::context::build_child_initial_messages).
/// The system prompt is already included as the first message — the launcher
/// must **not** set it again in the runtime config.
#[async_trait]
pub(super) trait SubagentLauncher: Send + Sync {
    async fn launch(
        &self,
        id: SubagentId,
        initial_messages: Vec<ModelMessage>,
        config: AgentConfig,
    ) -> Result<LaunchedChild, RociError>;
}

// ---------------------------------------------------------------------------
// In-process launcher
// ---------------------------------------------------------------------------

/// Launches child sub-agents as in-process [`AgentRuntime`] instances.
pub(super) struct InProcessLauncher {
    pub registry: Arc<ProviderRegistry>,
    pub roci_config: RociConfig,
}

#[async_trait]
impl SubagentLauncher for InProcessLauncher {
    async fn launch(
        &self,
        _id: SubagentId,
        initial_messages: Vec<ModelMessage>,
        config: AgentConfig,
    ) -> Result<LaunchedChild, RociError> {
        let runtime = AgentRuntime::new(self.registry.clone(), self.roci_config.clone(), config);

        // Seed the child runtime with the fully-composed message list.
        // The system prompt is the first message; the config has no system
        // prompt so `prompt()` won't duplicate it.
        if !initial_messages.is_empty() {
            runtime.replace_messages(initial_messages).await?;
        }

        Ok(LaunchedChild { runtime })
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Build a child [`AgentConfig`] from an explicit inheritance matrix.
///
/// Matrix:
/// - `system_prompt`: reset. The composed prompt lives in `initial_messages`.
/// - `event_sink`: replace with child forwarding sink.
/// - hooks/callbacks: reset (`transform_context`, `convert_to_llm`,
///   `before_agent_start`, session hooks, tool hooks, provider payload callback).
/// - approval policy/handler: inherit from parent.
/// - session fields: reset `session_id`.
/// - provider fields: inherit transport, retry, API key, headers, metadata, key fn.
/// - tools: replace with profile-selected child tools.
/// - user input coordinator: replace with supervisor coordinator.
/// - compaction: inherit. Chat config: reset to avoid sharing parent event store.
pub(super) fn build_child_config(
    parent: &AgentConfig,
    model: LanguageModel,
    tools: Vec<Arc<dyn Tool>>,
    reasoning_effort: Option<&str>,
    event_sink: Option<AgentEventSink>,
    #[cfg(feature = "agent")] coordinator: Arc<HumanInteractionCoordinator>,
) -> Result<AgentConfig, RociError> {
    let mut settings = parent.settings.clone();
    if let Some(reasoning_effort) = reasoning_effort {
        settings.reasoning_effort =
            Some(ReasoningEffort::from_str(reasoning_effort).map_err(|_| {
                RociError::Configuration(format!(
                    "invalid reasoning_effort '{reasoning_effort}' in subagent model candidate"
                ))
            })?);
    }

    Ok(AgentConfig {
        model,
        system_prompt: None,
        tools,
        tool_visibility_policy: parent.tool_visibility_policy.clone(),
        event_sink,
        approval_policy: parent.approval_policy,
        approval_handler: parent.approval_handler.clone(),
        #[cfg(feature = "agent")]
        human_interaction_coordinator: Some(coordinator),
        dynamic_tool_providers: Vec::new(),
        settings,
        transform_context: None,
        convert_to_llm: None,
        before_agent_start: None,
        session_id: None,
        steering_mode: parent.steering_mode,
        follow_up_mode: parent.follow_up_mode,
        transport: parent.transport.clone(),
        max_retry_delay_ms: parent.max_retry_delay_ms,
        retry_backoff: parent.retry_backoff,
        api_key_override: parent.api_key_override.clone(),
        provider_headers: parent.provider_headers.clone(),
        provider_metadata: parent.provider_metadata.clone(),
        provider_payload_callback: None,
        get_api_key: parent.get_api_key.clone(),
        compaction: parent.compaction.clone(),
        session_before_compact: None,
        session_before_tree: None,
        pre_tool_use: None,
        post_tool_use: None,
        user_input_timeout_ms: parent.user_input_timeout_ms,
        context_budget: parent.context_budget.clone(),
        chat: Default::default(),
    })
}

pub(super) fn select_child_tools(
    parent_tools: &[Arc<dyn Tool>],
    policy: &ToolPolicy,
) -> Result<Vec<Arc<dyn Tool>>, RociError> {
    match policy {
        ToolPolicy::Inherit => Ok(parent_tools.to_vec()),
        ToolPolicy::Replace { tools } => tools
            .iter()
            .map(|name| find_parent_tool(parent_tools, name))
            .collect(),
        ToolPolicy::InheritWithOverrides { add, remove } => {
            for name in remove {
                find_parent_tool(parent_tools, name)?;
            }
            let mut selected: Vec<_> = parent_tools
                .iter()
                .filter(|tool| !remove.iter().any(|name| name == tool.name()))
                .cloned()
                .collect();
            for name in add {
                if selected.iter().any(|tool| tool.name() == name) {
                    continue;
                }
                selected.push(find_parent_tool(parent_tools, name)?);
            }
            Ok(selected)
        }
    }
}

fn find_parent_tool(
    parent_tools: &[Arc<dyn Tool>],
    name: &str,
) -> Result<Arc<dyn Tool>, RociError> {
    parent_tools
        .iter()
        .find(|tool| tool.name() == name)
        .cloned()
        .ok_or_else(|| RociError::Configuration(format!("subagent tool '{name}' is not available")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::agent_loop::runner::{
        BeforeAgentStartHookResult, PreToolUseHookResult, TransformContextHookResult,
    };
    use crate::agent_loop::ApprovalPolicy;
    use crate::tools::{AgentTool, AgentToolParameters};
    use crate::types::GenerationSettings;

    #[test]
    fn build_child_config_has_no_system_prompt() {
        let model = LanguageModel::Known {
            provider_key: "test".into(),
            model_id: "test-model".into(),
        };
        let parent = AgentConfig {
            system_prompt: Some("parent prompt".into()),
            ..AgentConfig::default()
        };
        let cfg = build_child_config(
            &parent,
            model.clone(),
            Vec::new(),
            None,
            None,
            #[cfg(feature = "agent")]
            Arc::new(HumanInteractionCoordinator::new()),
        )
        .unwrap();
        assert_eq!(cfg.model, model);
        assert!(
            cfg.system_prompt.is_none(),
            "system prompt must be None; it lives in initial_messages"
        );
        assert!(cfg.tools.is_empty());
    }

    #[test]
    fn build_child_config_applies_explicit_inheritance_matrix() {
        let mut provider_headers = reqwest::header::HeaderMap::new();
        provider_headers.insert("x-parent", "1".parse().unwrap());
        let parent = AgentConfig {
            system_prompt: Some("parent prompt".into()),
            event_sink: Some(Arc::new(|_| {})),
            transform_context: Some(Arc::new(|_| {
                Box::pin(async { Ok(TransformContextHookResult::Continue) })
            })),
            before_agent_start: Some(Arc::new(|_| {
                Box::pin(async { Ok(BeforeAgentStartHookResult::Continue) })
            })),
            pre_tool_use: Some(Arc::new(|_, _| {
                Box::pin(async { Ok(PreToolUseHookResult::Continue) })
            })),
            approval_policy: ApprovalPolicy::Never,
            session_id: Some("parent-session".into()),
            transport: Some("proxy".into()),
            max_retry_delay_ms: Some(123),
            api_key_override: Some("parent-key".into()),
            provider_headers,
            provider_metadata: HashMap::from([("tenant".into(), "parent".into())]),
            user_input_timeout_ms: Some(456),
            settings: GenerationSettings {
                temperature: Some(0.2),
                ..GenerationSettings::default()
            },
            ..AgentConfig::default()
        };
        let model = LanguageModel::Known {
            provider_key: "test".into(),
            model_id: "test-model".into(),
        };

        let cfg = build_child_config(
            &parent,
            model,
            Vec::new(),
            Some("medium"),
            None,
            #[cfg(feature = "agent")]
            Arc::new(HumanInteractionCoordinator::new()),
        )
        .unwrap();

        assert_eq!(cfg.system_prompt, None, "system_prompt resets");
        assert!(cfg.event_sink.is_none(), "event_sink is replacement-only");
        assert_eq!(cfg.approval_policy, ApprovalPolicy::Never);
        assert_eq!(cfg.session_id, None, "session_id resets");
        assert_eq!(cfg.transport.as_deref(), Some("proxy"));
        assert_eq!(cfg.max_retry_delay_ms, Some(123));
        assert_eq!(cfg.api_key_override.as_deref(), Some("parent-key"));
        assert_eq!(cfg.provider_metadata.get("tenant"), Some(&"parent".into()));
        assert_eq!(cfg.provider_headers.get("x-parent").unwrap(), "1");
        assert!(cfg.provider_payload_callback.is_none());
        assert_eq!(cfg.user_input_timeout_ms, Some(456));
        assert_eq!(cfg.settings.temperature, Some(0.2));
        assert_eq!(cfg.settings.reasoning_effort, Some(ReasoningEffort::Medium));
        assert!(cfg.dynamic_tool_providers.is_empty());
        assert!(cfg.transform_context.is_none());
        assert!(cfg.convert_to_llm.is_none());
        assert!(cfg.before_agent_start.is_none());
        assert!(cfg.session_before_compact.is_none());
        assert!(cfg.session_before_tree.is_none());
        assert!(cfg.pre_tool_use.is_none());
        assert!(cfg.post_tool_use.is_none());
        assert_eq!(cfg.chat, Default::default(), "chat config resets");
    }

    #[test]
    fn build_child_config_rejects_invalid_reasoning_effort() {
        let result = build_child_config(
            &AgentConfig::default(),
            LanguageModel::Known {
                provider_key: "test".into(),
                model_id: "test-model".into(),
            },
            Vec::new(),
            Some("maximum"),
            None,
            #[cfg(feature = "agent")]
            Arc::new(HumanInteractionCoordinator::new()),
        );
        let err = match result {
            Ok(_) => panic!("expected invalid reasoning_effort to fail"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("invalid reasoning_effort"));
    }

    #[test]
    fn select_child_tools_applies_tool_policy() {
        let parent = vec![
            dummy_tool("read"),
            dummy_tool("write"),
            dummy_tool("shell"),
            dummy_tool("search"),
        ];

        let replaced = select_child_tools(
            &parent,
            &ToolPolicy::Replace {
                tools: vec!["read".into(), "shell".into()],
            },
        )
        .unwrap();
        assert_eq!(tool_names(&replaced), vec!["read", "shell"]);

        let reshaped = select_child_tools(
            &parent,
            &ToolPolicy::InheritWithOverrides {
                add: vec!["write".into()],
                remove: vec!["shell".into(), "write".into()],
            },
        )
        .unwrap();
        assert_eq!(tool_names(&reshaped), vec!["read", "search", "write"]);
    }

    #[test]
    fn select_child_tools_rejects_unknown_replace_tool() {
        let parent = vec![dummy_tool("read")];
        let result = select_child_tools(
            &parent,
            &ToolPolicy::Replace {
                tools: vec!["missing".into()],
            },
        );
        let err = match result {
            Ok(_) => panic!("expected missing tool to fail"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn select_child_tools_rejects_unknown_remove_tool() {
        let parent = vec![dummy_tool("read")];
        let result = select_child_tools(
            &parent,
            &ToolPolicy::InheritWithOverrides {
                add: Vec::new(),
                remove: vec!["missing".into()],
            },
        );
        let err = match result {
            Ok(_) => panic!("expected missing remove tool to fail"),
            Err(err) => err,
        };

        assert!(err
            .to_string()
            .contains("subagent tool 'missing' is not available"));
    }

    fn dummy_tool(name: &str) -> Arc<dyn Tool> {
        Arc::new(AgentTool::new(
            name,
            "test tool",
            AgentToolParameters::empty(),
            |_args, _ctx| async { Ok(serde_json::Value::Null) },
        ))
    }

    fn tool_names(tools: &[Arc<dyn Tool>]) -> Vec<&str> {
        tools.iter().map(|tool| tool.name()).collect()
    }
}
