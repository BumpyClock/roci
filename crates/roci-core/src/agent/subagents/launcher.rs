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
use crate::types::GenerationSettings;

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
#[async_trait]
pub(super) trait SubagentLauncher: Send + Sync {
    async fn launch(
        &self,
        id: SubagentId,
        model: LanguageModel,
        system_prompt: Option<String>,
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
        system_prompt: Option<String>,
        tools: Vec<Arc<dyn Tool>>,
        #[cfg(feature = "agent")] coordinator: Arc<UserInputCoordinator>,
        event_sink: Option<AgentEventSink>,
    ) -> Result<LaunchedChild, RociError> {
        let config = build_child_config(
            model,
            system_prompt,
            tools,
            event_sink,
            #[cfg(feature = "agent")]
            coordinator,
        );
        let runtime = AgentRuntime::new(self.registry.clone(), self.roci_config.clone(), config);
        Ok(LaunchedChild { runtime })
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn build_child_config(
    model: LanguageModel,
    system_prompt: Option<String>,
    tools: Vec<Arc<dyn Tool>>,
    event_sink: Option<AgentEventSink>,
    #[cfg(feature = "agent")] coordinator: Arc<UserInputCoordinator>,
) -> AgentConfig {
    use crate::agent::runtime::QueueDrainMode;

    AgentConfig {
        model,
        system_prompt,
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_child_config_sets_model_and_prompt() {
        let model = LanguageModel::Known {
            provider_key: "test".into(),
            model_id: "test-model".into(),
        };
        let cfg = build_child_config(
            model.clone(),
            Some("system".into()),
            Vec::new(),
            None,
            #[cfg(feature = "agent")]
            Arc::new(UserInputCoordinator::new()),
        );
        assert_eq!(cfg.model, model);
        assert_eq!(cfg.system_prompt.as_deref(), Some("system"));
        assert!(cfg.tools.is_empty());
    }
}
