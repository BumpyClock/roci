//! Internal launcher trait and in-process implementation for child runtimes.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

#[cfg(feature = "agent")]
use crate::agent::runtime::UserInputCoordinator;
use crate::agent::runtime::{AgentConfig, AgentRuntime};
use crate::agent_loop::runner::{AgentEventSink, RetryBackoffPolicy};
use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider::ProviderRegistry;
use crate::resource::CompactionSettings;
use crate::tools::tool::Tool;
use crate::types::{GenerationSettings, ModelMessage};

use super::types::SubagentId;

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
        model: LanguageModel,
        initial_messages: Vec<ModelMessage>,
        tools: Vec<Arc<dyn Tool>>,
        #[cfg(feature = "agent")] coordinator: Arc<UserInputCoordinator>,
        event_sink: Option<AgentEventSink>,
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
        model: LanguageModel,
        initial_messages: Vec<ModelMessage>,
        tools: Vec<Arc<dyn Tool>>,
        #[cfg(feature = "agent")] coordinator: Arc<UserInputCoordinator>,
        event_sink: Option<AgentEventSink>,
    ) -> Result<LaunchedChild, RociError> {
        let config = build_child_config(
            model,
            tools,
            event_sink,
            #[cfg(feature = "agent")]
            coordinator,
        );
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

/// Build a child [`AgentConfig`] without a system prompt.
///
/// The composed system prompt lives in `initial_messages` (first message),
/// so `system_prompt` is intentionally `None` to prevent double-application
/// if `prompt()` were ever called on the child runtime.
fn build_child_config(
    model: LanguageModel,
    tools: Vec<Arc<dyn Tool>>,
    event_sink: Option<AgentEventSink>,
    #[cfg(feature = "agent")] coordinator: Arc<UserInputCoordinator>,
) -> AgentConfig {
    use crate::agent::runtime::QueueDrainMode;

    AgentConfig {
        model,
        system_prompt: None,
        tools,
        event_sink,
        #[cfg(feature = "agent")]
        user_input_coordinator: Some(coordinator),
        dynamic_tool_providers: Vec::new(),
        settings: GenerationSettings::default(),
        transform_context: None,
        convert_to_llm: None,
        before_agent_start: None,
        session_id: None,
        steering_mode: QueueDrainMode::All,
        follow_up_mode: QueueDrainMode::All,
        transport: None,
        max_retry_delay_ms: None,
        retry_backoff: RetryBackoffPolicy::default(),
        api_key_override: None,
        provider_headers: reqwest::header::HeaderMap::new(),
        provider_metadata: HashMap::new(),
        provider_payload_callback: None,
        get_api_key: None,
        compaction: CompactionSettings::default(),
        session_before_compact: None,
        session_before_tree: None,
        pre_tool_use: None,
        post_tool_use: None,
        user_input_timeout_ms: None,
        context_budget: None,
        chat: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_child_config_has_no_system_prompt() {
        let model = LanguageModel::Known {
            provider_key: "test".into(),
            model_id: "test-model".into(),
        };
        let cfg = build_child_config(
            model.clone(),
            Vec::new(),
            None,
            #[cfg(feature = "agent")]
            Arc::new(UserInputCoordinator::new()),
        );
        assert_eq!(cfg.model, model);
        assert!(
            cfg.system_prompt.is_none(),
            "system prompt must be None; it lives in initial_messages"
        );
        assert!(cfg.tools.is_empty());
    }
}
