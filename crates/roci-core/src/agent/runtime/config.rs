use std::collections::HashMap;
use std::sync::Arc;

use crate::agent_loop::events::RetryMode;
use crate::agent_loop::runner::{
    AgentEventSink, BeforeAgentStartHook, ConvertToLlmFn, PostToolUseHook, PreToolUseHook,
    RetryBackoffPolicy, TransformContextFn,
};
use crate::agent_loop::{ApprovalHandler, ApprovalPolicy};
use crate::context::ContextBudget;
use crate::error::RociError;
use crate::models::{LanguageModel, SharedModelHealthRegistry};
use crate::provider::ProviderPayloadCallback;
use crate::resource::CompactionSettings;
use crate::session::SessionConfig;
use crate::tools::catalog::ToolVisibilityPolicy;
use crate::tools::dynamic::DynamicToolProvider;
use crate::tools::tool::{SandboxProvider, Tool};
use crate::types::GenerationSettings;

use super::chat::ChatRuntimeConfig;
use super::types::{GetApiKeyFn, QueueDrainMode, SessionBeforeCompactHook, SessionBeforeTreeHook};

/// Configuration for creating an [`super::AgentRuntime`].
///
/// # API key resolution
///
/// By default, the agent resolves API keys automatically through the
/// [`crate::config::RociConfig`] passed to [`super::AgentRuntime::new`].
/// `crate::config::RociConfig` checks explicit API keys loaded from
/// environment or `.env`, then OAuth tokens saved via `roci auth login`.
///
/// Set [`get_api_key`](Self::get_api_key) only when you need per-request
/// dynamic keys (e.g., token rotation or multi-tenant key injection).
#[derive(Clone)]
pub struct AgentConfig {
    /// Ordered language model candidates to use for generation.
    pub candidates: Vec<LanguageModel>,
    /// Optional system prompt prepended to the first turn.
    pub system_prompt: Option<String>,
    /// Tools available for tool-use loops.
    pub tools: Vec<Arc<dyn Tool>>,
    /// Policy deciding which resolved tools are visible to the model.
    pub tool_visibility_policy: ToolVisibilityPolicy,
    /// Dynamic tool providers queried at run start.
    pub dynamic_tool_providers: Vec<Arc<dyn DynamicToolProvider>>,
    /// Generation settings (temperature, max_tokens, etc.).
    pub settings: GenerationSettings,
    /// Optional hook to transform the message context before each LLM call.
    pub transform_context: Option<TransformContextFn>,
    /// Optional hook to convert/filter agent-level messages before provider requests.
    pub convert_to_llm: Option<ConvertToLlmFn>,
    /// Optional lifecycle hook called before starting the runner.
    pub before_agent_start: Option<BeforeAgentStartHook>,
    /// Optional sink for high-level [`crate::agent_loop::AgentEvent`] emission.
    pub event_sink: Option<AgentEventSink>,
    /// Tool approval policy for each run.
    pub approval_policy: ApprovalPolicy,
    /// Optional host-owned approval resolver.
    pub approval_handler: Option<ApprovalHandler>,
    /// Optional session ID for provider-side prompt caching.
    pub session_id: Option<String>,
    /// Optional durable local session configuration.
    pub session: Option<SessionConfig>,
    /// Optional sandbox provider exposed to command-capable tools.
    pub sandbox_provider: Option<Arc<dyn SandboxProvider>>,
    /// Drain mode for steering queue retrieval.
    pub steering_mode: QueueDrainMode,
    /// Drain mode for follow-up queue retrieval.
    pub follow_up_mode: QueueDrainMode,
    /// Optional provider transport preference.
    pub transport: Option<String>,
    /// Optional cap for server-requested retry delays in milliseconds.
    /// `Some(0)` disables the cap.
    pub max_retry_delay_ms: Option<u64>,
    /// Retry/backoff policy for transient provider failures.
    pub retry_backoff: RetryBackoffPolicy,
    /// Optional retry mode override. When `None`, bounded retry attempts derive from retry_backoff.
    pub retry_mode: Option<RetryMode>,
    /// Shared model health registry. Each run creates a fresh session tracker.
    pub model_health: Arc<SharedModelHealthRegistry>,
    /// Optional per-run provider API key override.
    pub api_key_override: Option<String>,
    /// Optional per-run provider header overrides.
    pub provider_headers: reqwest::header::HeaderMap,
    /// Optional per-run provider metadata.
    pub provider_metadata: HashMap<String, String>,
    /// Optional callback for inspecting provider request payloads.
    pub provider_payload_callback: Option<ProviderPayloadCallback>,
    /// Optional async callback to resolve an API key for each active model.
    ///
    /// Precedence is: provider-scoped request override -> provider/config key -> this callback.
    ///
    /// When `None` (the default), the agent resolves keys automatically
    /// through [`crate::config::RociConfig`] which checks explicit API keys
    /// loaded from environment or `.env`, then OAuth tokens saved via
    /// `roci auth login`.
    /// No explicit key configuration is needed if any of those sources
    /// has a valid credential for the provider. The callback receives the
    /// active model so fallback across providers can resolve the right key.
    pub get_api_key: Option<GetApiKeyFn>,
    /// Compaction policy and summarization model selection.
    pub compaction: CompactionSettings,
    /// Optional lifecycle hook for `session_before_compact`.
    pub session_before_compact: Option<SessionBeforeCompactHook>,
    /// Optional lifecycle hook for `session_before_tree`.
    pub session_before_tree: Option<SessionBeforeTreeHook>,
    /// Optional hook called before each tool execution.
    pub pre_tool_use: Option<PreToolUseHook>,
    /// Optional hook called after each tool execution (including synthetic errors).
    pub post_tool_use: Option<PostToolUseHook>,
    /// Default timeout for user input requests in milliseconds.
    pub user_input_timeout_ms: Option<u64>,
    /// Optional context budget for per-turn and per-session token limits.
    ///
    /// When set, enables preflight budget enforcement using the session
    /// usage ledger maintained by the runtime. Each provider call is
    /// checked against per-turn and cumulative session limits before
    /// streaming begins.
    pub context_budget: Option<ContextBudget>,
    /// Chat runtime contract and event configuration.
    pub chat: ChatRuntimeConfig,
    /// Optional shared coordinator for human interaction requests.
    ///
    /// When provided, the runtime uses this coordinator instead of creating
    /// its own. This allows the CLI/host to share the coordinator and submit
    /// responses directly.
    #[cfg(feature = "agent")]
    pub human_interaction_coordinator:
        Option<std::sync::Arc<crate::human_interaction::HumanInteractionCoordinator>>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            candidates: vec![LanguageModel::Known {
                provider_key: "openai".to_string(),
                model_id: "gpt-4o".to_string(),
            }],
            system_prompt: None,
            tools: Vec::new(),
            tool_visibility_policy: ToolVisibilityPolicy::default(),
            dynamic_tool_providers: Vec::new(),
            settings: GenerationSettings::default(),
            transform_context: None,
            convert_to_llm: None,
            before_agent_start: None,
            event_sink: None,
            approval_policy: ApprovalPolicy::Ask,
            approval_handler: None,
            session_id: None,
            session: None,
            sandbox_provider: None,
            steering_mode: QueueDrainMode::All,
            follow_up_mode: QueueDrainMode::All,
            transport: None,
            max_retry_delay_ms: None,
            retry_backoff: RetryBackoffPolicy::default(),
            retry_mode: None,
            model_health: Arc::new(SharedModelHealthRegistry::default()),
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
            chat: ChatRuntimeConfig::default(),
            #[cfg(feature = "agent")]
            human_interaction_coordinator: None,
        }
    }
}

impl AgentConfig {
    pub(crate) fn effective_retry_mode(&self) -> Result<RetryMode, RociError> {
        match self.retry_mode {
            Some(RetryMode::Bounded { max_attempts: 0 }) => Err(RociError::Configuration(
                "retry_mode bounded max_attempts must be at least 1".to_string(),
            )),
            Some(mode) => Ok(mode),
            None if self.retry_backoff.max_attempts == 0 => Err(RociError::Configuration(
                "retry_backoff max_attempts must be at least 1".to_string(),
            )),
            None => Ok(RetryMode::Bounded {
                max_attempts: self.retry_backoff.max_attempts,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_retry_mode_derives_from_backoff() {
        assert_eq!(
            AgentConfig::default().effective_retry_mode().unwrap(),
            RetryMode::Bounded { max_attempts: 3 }
        );
    }

    #[test]
    fn explicit_retry_mode_overrides_backoff_attempts() {
        let config = AgentConfig {
            retry_backoff: RetryBackoffPolicy {
                max_attempts: 9,
                ..RetryBackoffPolicy::default()
            },
            retry_mode: Some(RetryMode::Bounded { max_attempts: 2 }),
            ..AgentConfig::default()
        };

        assert_eq!(
            config.effective_retry_mode().unwrap(),
            RetryMode::Bounded { max_attempts: 2 }
        );
    }

    #[test]
    fn derived_retry_mode_rejects_zero_backoff_attempts() {
        let config = AgentConfig {
            retry_backoff: RetryBackoffPolicy {
                max_attempts: 0,
                ..RetryBackoffPolicy::default()
            },
            ..AgentConfig::default()
        };

        assert!(matches!(
            config.effective_retry_mode(),
            Err(RociError::Configuration(_))
        ));
    }
}
