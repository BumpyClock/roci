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
        initial_messages: Vec<ModelMessage>,
        config: AgentConfig,
    ) -> Result<LaunchedChild, RociError> {
        let runtime =
            AgentRuntime::try_new(self.registry.clone(), self.roci_config.clone(), config)?;

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
/// - approval policy/handler: clone parent ruleset and inherit handler.
/// - session fields: reset. Child persistence needs child-specific session resources.
/// - workspace/sandbox: inherit parent host boundaries.
/// - provider fields: inherit dynamic providers, transport, retry, API key, headers,
///   metadata, and key fn.
/// - tools: retain the supplied native catalog so names remain reserved during
///   dynamic discovery; the supervisor applies profile restrictions to visibility.
/// - user input coordinator: replace with supervisor coordinator.
/// - compaction: inherit. Chat config: reset to avoid sharing parent event store.
pub(super) fn build_child_config(
    parent: &AgentConfig,
    candidates: Vec<LanguageModel>,
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
        candidates,
        system_prompt: None,
        tools,
        tool_visibility_policy: parent.tool_visibility_policy.clone(),
        event_sink,
        approval_policy: parent.approval_policy.clone(),
        approval_handler: parent.approval_handler.clone(),
        #[cfg(feature = "agent")]
        human_interaction_coordinator: Some(coordinator),
        dynamic_tool_providers: parent.dynamic_tool_providers.clone(),
        settings,
        transform_context: None,
        convert_to_llm: None,
        before_agent_start: None,
        session_id: None,
        session: None,
        workspace_root: parent.workspace_root.clone(),
        sandbox_provider: parent.sandbox_provider.clone(),
        steering_mode: parent.steering_mode,
        follow_up_mode: parent.follow_up_mode,
        transport: parent.transport.clone(),
        max_retry_delay_ms: parent.max_retry_delay_ms,
        retry_backoff: parent.retry_backoff,
        retry_mode: parent.retry_mode,
        model_health: parent.model_health.clone(),
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
        #[cfg(feature = "agent")]
        subagents: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::agent_loop::runner::{
        BeforeAgentStartHookResult, PreToolUseHookResult, TransformContextHookResult,
    };
    use crate::agent_loop::ApprovalPolicy;
    use crate::agent_loop::RetryMode;
    use crate::session::LogicalPath;
    use crate::tools::arguments::ToolArguments;
    use crate::tools::dynamic::{scope_dynamic_tool_providers, DynamicTool, DynamicToolProvider};
    use crate::tools::tool::ToolExecutionContext;
    use crate::tools::SandboxProvider;
    use crate::types::GenerationSettings;

    struct TestDynamicToolProvider {
        server_ids: Vec<String>,
    }

    struct TestSandboxProvider;

    #[tokio::test]
    async fn launch_returns_error_when_inherited_workspace_disappears() {
        let workspace = tempfile::tempdir().expect("workspace temp dir");
        let workspace_root = workspace.path().to_path_buf();
        drop(workspace);
        let launcher = InProcessLauncher {
            registry: Arc::new(ProviderRegistry::new()),
            roci_config: RociConfig::new(),
        };
        let config = AgentConfig {
            candidates: vec![LanguageModel::Known {
                provider_key: "test".to_string(),
                model_id: "test-model".to_string(),
            }],
            workspace_root: Some(workspace_root),
            ..AgentConfig::default()
        };

        let error = match launcher.launch(Vec::new(), config).await {
            Ok(_) => panic!("missing inherited workspace must fail child launch"),
            Err(error) => error,
        };

        assert!(matches!(error, RociError::Configuration(_)));
        assert!(error
            .to_string()
            .contains("failed to canonicalize workspace root"));
    }

    #[async_trait]
    impl SandboxProvider for TestSandboxProvider {
        async fn validate_shell_command(
            &self,
            _command: &str,
            _cwd: &LogicalPath,
        ) -> Result<(), RociError> {
            Ok(())
        }
    }

    #[async_trait]
    impl DynamicToolProvider for TestDynamicToolProvider {
        fn server_ids(&self) -> Vec<String> {
            self.server_ids.clone()
        }

        async fn list_tools(&self) -> Result<Vec<DynamicTool>, RociError> {
            Ok(Vec::new())
        }

        async fn execute_tool(
            &self,
            _name: &str,
            _args: &ToolArguments,
            _ctx: &ToolExecutionContext,
        ) -> Result<serde_json::Value, RociError> {
            Ok(serde_json::Value::Null)
        }
    }

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
            vec![model.clone()],
            Vec::new(),
            None,
            None,
            #[cfg(feature = "agent")]
            Arc::new(HumanInteractionCoordinator::new()),
        )
        .unwrap();
        assert_eq!(cfg.candidates, vec![model]);
        assert!(
            cfg.system_prompt.is_none(),
            "system prompt must be None; it lives in initial_messages"
        );
        assert!(cfg.tools.is_empty());
    }

    #[test]
    fn subagent_runtime_wiring_build_child_config_clears_subagents() {
        let model = LanguageModel::Known {
            provider_key: "test".into(),
            model_id: "test-model".into(),
        };
        let parent = AgentConfig {
            subagents: Some(Default::default()),
            ..AgentConfig::default()
        };

        let cfg = build_child_config(
            &parent,
            vec![model],
            Vec::new(),
            None,
            None,
            #[cfg(feature = "agent")]
            Arc::new(HumanInteractionCoordinator::new()),
        )
        .unwrap();

        assert!(cfg.subagents.is_none());
    }

    #[test]
    fn build_child_config_applies_explicit_inheritance_matrix() {
        let mut provider_headers = reqwest::header::HeaderMap::new();
        provider_headers.insert("x-parent", "1".parse().unwrap());
        let dynamic_provider: Arc<dyn DynamicToolProvider> = Arc::new(TestDynamicToolProvider {
            server_ids: vec!["github".to_string()],
        });
        let sandbox_provider: Arc<dyn SandboxProvider> = Arc::new(TestSandboxProvider);
        let workspace_root = std::env::temp_dir().join("roci-parent-workspace");
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
            approval_policy: ApprovalPolicy::never(),
            session_id: Some("parent-session".into()),
            session: Some(crate::session::SessionConfig::new(
                crate::session::SessionId::parse("parent-durable-session").unwrap(),
                std::env::temp_dir(),
            )),
            workspace_root: Some(workspace_root.clone()),
            sandbox_provider: Some(sandbox_provider.clone()),
            transport: Some("proxy".into()),
            max_retry_delay_ms: Some(123),
            retry_mode: Some(RetryMode::Persistent),
            api_key_override: Some("parent-key".into()),
            provider_headers,
            provider_metadata: HashMap::from([("tenant".into(), "parent".into())]),
            dynamic_tool_providers: vec![dynamic_provider.clone()],
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
            vec![model],
            Vec::new(),
            Some("medium"),
            None,
            #[cfg(feature = "agent")]
            Arc::new(HumanInteractionCoordinator::new()),
        )
        .unwrap();

        assert_eq!(cfg.system_prompt, None, "system_prompt resets");
        assert!(cfg.event_sink.is_none(), "event_sink is replacement-only");
        assert_eq!(cfg.approval_policy, ApprovalPolicy::never());
        assert_eq!(cfg.session_id, None, "session_id resets");
        assert_eq!(cfg.session, None, "durable session config resets");
        assert_eq!(cfg.workspace_root, Some(workspace_root));
        assert!(Arc::ptr_eq(
            cfg.sandbox_provider.as_ref().unwrap(),
            &sandbox_provider
        ));
        assert_eq!(cfg.transport.as_deref(), Some("proxy"));
        assert_eq!(cfg.max_retry_delay_ms, Some(123));
        assert_eq!(cfg.retry_mode, Some(RetryMode::Persistent));
        assert_eq!(cfg.api_key_override.as_deref(), Some("parent-key"));
        assert_eq!(cfg.provider_metadata.get("tenant"), Some(&"parent".into()));
        assert_eq!(cfg.provider_headers.get("x-parent").unwrap(), "1");
        assert!(cfg.provider_payload_callback.is_none());
        assert_eq!(cfg.user_input_timeout_ms, Some(456));
        assert_eq!(cfg.settings.temperature, Some(0.2));
        assert_eq!(cfg.settings.reasoning_effort, Some(ReasoningEffort::Medium));
        assert_eq!(cfg.dynamic_tool_providers.len(), 1);
        assert!(Arc::ptr_eq(
            &cfg.dynamic_tool_providers[0],
            &dynamic_provider
        ));
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
    fn select_child_dynamic_providers_scopes_explicit_mcp_servers() {
        let multi_server: Arc<dyn DynamicToolProvider> = Arc::new(TestDynamicToolProvider {
            server_ids: vec!["github".into(), "linear".into()],
        });
        let unrelated: Arc<dyn DynamicToolProvider> = Arc::new(TestDynamicToolProvider {
            server_ids: vec!["figma".into()],
        });

        let selected =
            scope_dynamic_tool_providers(&[multi_server, unrelated], &["linear".to_string()])
                .unwrap();

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].server_ids(), vec!["linear"]);
    }

    #[test]
    fn select_child_dynamic_providers_empty_allow_list_selects_none() {
        let provider: Arc<dyn DynamicToolProvider> = Arc::new(TestDynamicToolProvider {
            server_ids: vec!["github".into()],
        });

        let selected = scope_dynamic_tool_providers(&[provider], &[]).unwrap();

        assert!(selected.is_empty());
    }

    #[test]
    fn select_child_dynamic_providers_rejects_unknown_mcp_servers() {
        let provider: Arc<dyn DynamicToolProvider> = Arc::new(TestDynamicToolProvider {
            server_ids: vec!["github".into()],
        });

        let result = scope_dynamic_tool_providers(
            &[provider],
            &["github".to_string(), "missing".to_string()],
        );
        let error = match result {
            Ok(_) => panic!("expected unknown MCP server id to fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("missing"));
    }

    #[test]
    fn build_child_config_rejects_invalid_reasoning_effort() {
        let result = build_child_config(
            &AgentConfig::default(),
            vec![LanguageModel::Known {
                provider_key: "test".into(),
                model_id: "test-model".into(),
            }],
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
}
