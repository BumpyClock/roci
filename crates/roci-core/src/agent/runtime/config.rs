use std::collections::HashMap;
use std::sync::Arc;

use crate::agent_loop::runner::{
    AgentEventSink, BeforeAgentStartHook, ConvertToLlmFn, PostToolUseHook, PreToolUseHook,
    RetryBackoffPolicy, TransformContextFn,
};
use crate::agent_loop::{ApprovalHandler, ApprovalPolicy};
use crate::context::ContextBudget;
use crate::models::LanguageModel;
use crate::provider::ProviderPayloadCallback;
use crate::resource::CompactionSettings;
use crate::tools::dynamic::DynamicToolProvider;
use crate::tools::tool::Tool;
use crate::types::GenerationSettings;

use super::chat::ChatRuntimeConfig;
use super::types::{GetApiKeyFn, QueueDrainMode, SessionBeforeCompactHook, SessionBeforeTreeHook};

/// Configuration for creating an [`super::AgentRuntime`].
///
/// # API key resolution
///
/// By default, the agent resolves API keys automatically through the
/// [`crate::config::RociConfig`] passed to [`super::AgentRuntime::new`].
/// `crate::config::RociConfig` checks (in order): environment variables →
/// `credentials.json` → OAuth token store (from `roci auth login`).
///
/// Set [`get_api_key`](Self::get_api_key) only when you need per-request
/// dynamic keys (e.g., token rotation or multi-tenant key injection).
#[derive(Clone)]
pub struct AgentConfig {
    /// The language model to use for generation.
    pub model: LanguageModel,
    /// Optional system prompt prepended to the first turn.
    pub system_prompt: Option<String>,
    /// Tools available for tool-use loops.
    pub tools: Vec<Arc<dyn Tool>>,
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
    /// Optional per-run provider API key override.
    pub api_key_override: Option<String>,
    /// Optional per-run provider header overrides.
    pub provider_headers: reqwest::header::HeaderMap,
    /// Optional per-run provider metadata.
    pub provider_metadata: HashMap<String, String>,
    /// Optional callback for inspecting provider request payloads.
    pub provider_payload_callback: Option<ProviderPayloadCallback>,
    /// Optional async callback to resolve an API key per run.
    ///
    /// Precedence is: request override -> provider/config key -> this callback.
    ///
    /// When `None` (the default), the agent resolves keys automatically
    /// through [`crate::config::RociConfig`] which checks: environment variables →
    /// `credentials.json` → OAuth token store (from `roci auth login`).
    /// No explicit key configuration is needed if any of those sources
    /// has a valid credential for the provider.
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
    /// Optional shared coordinator for user input requests.
    ///
    /// When provided, the runtime uses this coordinator instead of creating
    /// its own. This allows the CLI/host to share the coordinator and submit
    /// responses directly.
    #[cfg(feature = "agent")]
    pub user_input_coordinator: Option<std::sync::Arc<super::UserInputCoordinator>>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: LanguageModel::Known {
                provider_key: "openai".to_string(),
                model_id: "gpt-4o".to_string(),
            },
            system_prompt: None,
            tools: Vec::new(),
            dynamic_tool_providers: Vec::new(),
            settings: GenerationSettings::default(),
            transform_context: None,
            convert_to_llm: None,
            before_agent_start: None,
            event_sink: None,
            approval_policy: ApprovalPolicy::Ask,
            approval_handler: None,
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
            chat: ChatRuntimeConfig::default(),
            #[cfg(feature = "agent")]
            user_input_coordinator: None,
        }
    }
}
