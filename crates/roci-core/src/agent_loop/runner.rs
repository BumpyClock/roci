//! Runner interfaces for the agent loop.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::message::AgentMessage;
use crate::config::RociConfig;
use crate::context::ContextBudget;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider::{self, ProviderRegistry};
use crate::tools::tool::Tool;
use crate::types::{AgentToolCall, AgentToolResult, GenerationSettings, ModelMessage};

use super::approvals::{ApprovalDecision, ApprovalHandler, ApprovalPolicy};
use super::events::{AgentEvent, RunEvent, RunEventPayload, RunEventStream, RunLifecycle};
use super::types::{RunId, RunResult};

/// Callback used for streaming run events.
pub type RunEventSink = Arc<dyn Fn(RunEvent) + Send + Sync>;
/// Hook to compact/prune a message history before the next provider call.
pub type CompactionHandler = Arc<
    dyn Fn(
            Vec<ModelMessage>,
            CancellationToken,
        )
            -> Pin<Box<dyn Future<Output = Result<Option<Vec<ModelMessage>>, RociError>> + Send>>
        + Send
        + Sync,
>;
/// Decision returned by pre-tool-use hook.
#[derive(Debug, Clone, PartialEq)]
pub enum PreToolUseHookResult {
    Continue,
    Block { reason: Option<String> },
    ReplaceArgs { args: serde_json::Value },
}

/// Hook that can block or rewrite args before tool execution.
pub type PreToolUseHook = Arc<
    dyn Fn(
            AgentToolCall,
            CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Result<PreToolUseHookResult, RociError>> + Send>>
        + Send
        + Sync,
>;

/// Hook that can rewrite any tool result before persistence/context assembly.
pub type PostToolUseHook = Arc<
    dyn Fn(
            AgentToolCall,
            AgentToolResult,
        ) -> Pin<Box<dyn Future<Output = Result<AgentToolResult, RociError>> + Send>>
        + Send
        + Sync,
>;

/// Async callback to retrieve messages between loop phases.
pub type MessageBatchFn =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Vec<ModelMessage>> + Send>> + Send + Sync>;

/// Callback to retrieve steering messages between tool batches.
pub type SteeringMessagesFn = MessageBatchFn;

/// Callback to retrieve follow-up messages after the inner loop completes.
pub type FollowUpMessagesFn = MessageBatchFn;

/// Payload for the `before_agent_start` lifecycle hook.
#[derive(Debug, Clone)]
pub struct BeforeAgentStartHookPayload {
    pub run_id: RunId,
    pub model: LanguageModel,
    pub messages: Vec<ModelMessage>,
    pub cancellation_token: CancellationToken,
}

/// Decision returned by `before_agent_start`.
#[derive(Debug, Clone, PartialEq)]
pub enum BeforeAgentStartHookResult {
    Continue,
    Cancel { reason: Option<String> },
    ReplaceMessages { messages: Vec<ModelMessage> },
}

/// Hook called before runner startup to mutate/cancel initial run context.
pub type BeforeAgentStartHook = Arc<
    dyn Fn(
            BeforeAgentStartHookPayload,
        )
            -> Pin<Box<dyn Future<Output = Result<BeforeAgentStartHookResult, RociError>> + Send>>
        + Send
        + Sync,
>;

/// Payload for the `transform_context` hook.
#[derive(Debug, Clone)]
pub struct TransformContextHookPayload {
    pub run_id: RunId,
    pub model: LanguageModel,
    pub messages: Vec<ModelMessage>,
    pub cancellation_token: CancellationToken,
}

/// Decision returned by `transform_context`.
#[derive(Debug, Clone, PartialEq)]
pub enum TransformContextHookResult {
    Continue,
    Cancel { reason: Option<String> },
    ReplaceMessages { messages: Vec<ModelMessage> },
}

/// Hook to transform message context before `convert_to_llm` and provider sanitize.
pub type TransformContextFn = Arc<
    dyn Fn(
            TransformContextHookPayload,
        )
            -> Pin<Box<dyn Future<Output = Result<TransformContextHookResult, RociError>> + Send>>
        + Send
        + Sync,
>;

/// Payload for the `convert_to_llm` hook.
#[derive(Debug, Clone)]
pub struct ConvertToLlmHookPayload {
    pub run_id: RunId,
    pub model: LanguageModel,
    pub messages: Vec<AgentMessage>,
    pub cancellation_token: CancellationToken,
}

/// Decision returned by `convert_to_llm`.
#[derive(Debug, Clone, PartialEq)]
pub enum ConvertToLlmHookResult {
    Continue,
    Cancel { reason: Option<String> },
    ReplaceMessages { messages: Vec<ModelMessage> },
}

/// Hook to convert/filter agent-level messages into provider-facing LLM messages.
///
/// This runs after `transform_context` and before provider sanitization.
pub type ConvertToLlmFn = Arc<
    dyn Fn(
            ConvertToLlmHookPayload,
        )
            -> Pin<Box<dyn Future<Output = Result<ConvertToLlmHookResult, RociError>> + Send>>
        + Send
        + Sync,
>;

/// Sink for high-level AgentEvent emission (separate from RunEvent).
pub type AgentEventSink = Arc<dyn Fn(AgentEvent) + Send + Sync>;

#[derive(Clone, Default)]
pub struct RunHooks {
    pub compaction: Option<CompactionHandler>,
    pub pre_tool_use: Option<PreToolUseHook>,
    pub post_tool_use: Option<PostToolUseHook>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoCompactionConfig {
    pub reserve_tokens: usize,
}

/// Retry/backoff policy for retryable provider request failures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryBackoffPolicy {
    /// Maximum attempts including the first provider call.
    pub max_attempts: u32,
    /// Initial backoff delay in milliseconds.
    pub initial_delay_ms: u64,
    /// Exponential multiplier applied after each failed attempt.
    pub multiplier: f64,
    /// Symmetric jitter ratio applied around each computed delay (e.g. 0.2 => +/-20%).
    pub jitter_ratio: f64,
    /// Maximum bounded delay in milliseconds.
    pub max_delay_ms: u64,
}

impl Default for RetryBackoffPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay_ms: 250,
            multiplier: 2.0,
            jitter_ratio: 0.2,
            max_delay_ms: 2_000,
        }
    }
}

/// Request payload to start a run.
#[derive(Clone)]
pub struct RunRequest {
    pub run_id: RunId,
    pub model: LanguageModel,
    pub messages: Vec<ModelMessage>,
    pub settings: GenerationSettings,
    pub tools: Vec<Arc<dyn Tool>>,
    pub approval_policy: ApprovalPolicy,
    pub approval_handler: Option<ApprovalHandler>,
    pub metadata: HashMap<String, String>,
    pub event_sink: Option<RunEventSink>,
    pub hooks: RunHooks,
    pub auto_compaction: Option<AutoCompactionConfig>,
    /// Per-run retry/backoff policy for retryable provider failures.
    pub retry_backoff: RetryBackoffPolicy,
    /// Callback to get steering messages (checked between tool batches).
    pub get_steering_messages: Option<SteeringMessagesFn>,
    /// Callback to get follow-up messages (checked after inner loop ends).
    pub get_follow_up_messages: Option<FollowUpMessagesFn>,
    /// Pre-LLM context transformation hook.
    pub transform_context: Option<TransformContextFn>,
    /// Optional conversion/filter hook for agent-level messages.
    pub convert_to_llm: Option<ConvertToLlmFn>,
    /// AgentEvent sink (separate from RunEvent sink).
    pub agent_event_sink: Option<AgentEventSink>,
    /// Optional session ID for provider-side prompt caching.
    pub session_id: Option<String>,
    /// Optional provider transport preference.
    pub transport: Option<String>,
    /// Optional cap for server-requested retry delays in milliseconds.
    /// `Some(0)` disables the cap.
    pub max_retry_delay_ms: Option<u64>,
    /// Optional per-request provider API key override.
    pub api_key_override: Option<String>,
    /// Optional per-request provider header overrides.
    pub provider_headers: reqwest::header::HeaderMap,
    /// Optional per-request metadata passed to providers.
    pub provider_metadata: HashMap<String, String>,
    /// Optional per-request payload inspection callback.
    pub provider_payload_callback: Option<provider::ProviderPayloadCallback>,
    /// Optional context budget for preflight budget enforcement.
    pub context_budget: Option<ContextBudget>,
    /// Cumulative session input tokens from all previous runs (frozen at run start).
    pub prior_session_input_tokens: usize,
    /// Cumulative session output tokens from all previous runs (frozen at run start).
    pub prior_session_output_tokens: usize,
    /// Optional callback for requesting user input from tools.
    #[cfg(feature = "agent")]
    pub user_input_callback: Option<crate::tools::user_input::RequestUserInputFn>,
}

impl RunRequest {
    pub fn new(model: LanguageModel, messages: Vec<ModelMessage>) -> Self {
        Self {
            run_id: Uuid::new_v4(),
            model,
            messages,
            settings: GenerationSettings::default(),
            tools: Vec::new(),
            approval_policy: ApprovalPolicy::Ask,
            approval_handler: None,
            metadata: HashMap::new(),
            event_sink: None,
            hooks: RunHooks::default(),
            auto_compaction: None,
            retry_backoff: RetryBackoffPolicy::default(),
            get_steering_messages: None,
            get_follow_up_messages: None,
            transform_context: None,
            convert_to_llm: None,
            agent_event_sink: None,
            session_id: None,
            transport: None,
            max_retry_delay_ms: None,
            api_key_override: None,
            provider_headers: reqwest::header::HeaderMap::new(),
            provider_metadata: HashMap::new(),
            provider_payload_callback: None,
            context_budget: None,
            prior_session_input_tokens: 0,
            prior_session_output_tokens: 0,
            #[cfg(feature = "agent")]
            user_input_callback: None,
        }
    }

    pub fn with_tools(mut self, tools: Vec<Arc<dyn Tool>>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_event_sink(mut self, sink: RunEventSink) -> Self {
        self.event_sink = Some(sink);
        self
    }

    pub fn with_approval_policy(mut self, policy: ApprovalPolicy) -> Self {
        self.approval_policy = policy;
        self
    }

    pub fn with_approval_handler(mut self, handler: ApprovalHandler) -> Self {
        self.approval_handler = Some(handler);
        self
    }

    pub fn with_hooks(mut self, hooks: RunHooks) -> Self {
        self.hooks = hooks;
        self
    }

    pub fn with_auto_compaction(mut self, config: AutoCompactionConfig) -> Self {
        self.auto_compaction = Some(config);
        self
    }

    pub fn with_steering_messages(mut self, f: SteeringMessagesFn) -> Self {
        self.get_steering_messages = Some(f);
        self
    }

    pub fn with_follow_up_messages(mut self, f: FollowUpMessagesFn) -> Self {
        self.get_follow_up_messages = Some(f);
        self
    }

    pub fn with_transform_context(mut self, f: TransformContextFn) -> Self {
        self.transform_context = Some(f);
        self
    }

    pub fn with_convert_to_llm(mut self, f: ConvertToLlmFn) -> Self {
        self.convert_to_llm = Some(f);
        self
    }

    pub fn with_agent_event_sink(mut self, sink: AgentEventSink) -> Self {
        self.agent_event_sink = Some(sink);
        self
    }

    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    pub fn with_transport(mut self, transport: impl Into<String>) -> Self {
        self.transport = Some(transport.into());
        self
    }

    pub fn with_max_retry_delay_ms(mut self, max_retry_delay_ms: u64) -> Self {
        self.max_retry_delay_ms = Some(max_retry_delay_ms);
        self
    }

    pub fn with_retry_backoff(mut self, retry_backoff: RetryBackoffPolicy) -> Self {
        self.retry_backoff = retry_backoff;
        self
    }

    pub fn with_api_key_override(mut self, key: impl Into<String>) -> Self {
        self.api_key_override = Some(key.into());
        self
    }

    pub fn with_provider_headers(mut self, headers: reqwest::header::HeaderMap) -> Self {
        self.provider_headers = headers;
        self
    }

    pub fn with_provider_metadata(mut self, metadata: HashMap<String, String>) -> Self {
        self.provider_metadata = metadata;
        self
    }

    pub fn with_provider_payload_callback(
        mut self,
        callback: provider::ProviderPayloadCallback,
    ) -> Self {
        self.provider_payload_callback = Some(callback);
        self
    }

    pub fn with_context_budget(mut self, budget: ContextBudget) -> Self {
        self.context_budget = Some(budget);
        self
    }

    pub fn with_prior_session_usage(mut self, input_tokens: usize, output_tokens: usize) -> Self {
        self.prior_session_input_tokens = input_tokens;
        self.prior_session_output_tokens = output_tokens;
        self
    }

    #[cfg(feature = "agent")]
    pub fn with_user_input_callback(
        mut self,
        cb: crate::tools::user_input::RequestUserInputFn,
    ) -> Self {
        self.user_input_callback = Some(cb);
        self
    }
}

/// Handle for an in-flight run.
#[derive(Debug)]
pub struct RunHandle {
    run_id: RunId,
    abort_tx: Option<oneshot::Sender<()>>,
    result_rx: oneshot::Receiver<RunResult>,
    input_tx: Option<mpsc::UnboundedSender<ModelMessage>>,
}

impl RunHandle {
    /// Create a new run handle and expose internal channels to a runner implementation.
    pub fn new(
        run_id: RunId,
    ) -> (
        Self,
        oneshot::Receiver<()>,
        oneshot::Sender<RunResult>,
        mpsc::UnboundedReceiver<ModelMessage>,
    ) {
        let (abort_tx, abort_rx) = oneshot::channel();
        let (result_tx, result_rx) = oneshot::channel();
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        (
            Self {
                run_id,
                abort_tx: Some(abort_tx),
                result_rx,
                input_tx: Some(input_tx),
            },
            abort_rx,
            result_tx,
            input_rx,
        )
    }

    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    pub fn abort(&mut self) -> bool {
        if let Some(tx) = self.abort_tx.take() {
            return tx.send(()).is_ok();
        }
        false
    }

    pub fn take_abort_sender(&mut self) -> Option<oneshot::Sender<()>> {
        self.abort_tx.take()
    }

    pub fn queue_message(&self, message: ModelMessage) -> bool {
        if let Some(tx) = &self.input_tx {
            return tx.send(message).is_ok();
        }
        false
    }

    pub async fn wait(self) -> RunResult {
        self.result_rx
            .await
            .unwrap_or_else(|_| RunResult::canceled_with_messages(Vec::new()))
    }
}

/// Runner trait for executing agent loop requests.
#[async_trait]
pub trait Runner: Send + Sync {
    async fn start(&self, request: RunRequest) -> Result<RunHandle, RociError>;
}

/// Default agent-loop runner (tool loop + approvals + event stream).
pub struct LoopRunner {
    config: RociConfig,
    provider_factory: ProviderFactory,
}

impl LoopRunner {
    /// Create with an `Arc<ProviderRegistry>` for dynamic provider resolution.
    pub fn with_registry(config: RociConfig, registry: Arc<ProviderRegistry>) -> Self {
        Self {
            config,
            provider_factory: Arc::new(move |model, cfg| {
                registry.create_provider(model.provider_name(), model.model_id(), cfg)
            }),
        }
    }

    #[cfg(test)]
    fn with_provider_factory(config: RociConfig, provider_factory: ProviderFactory) -> Self {
        Self {
            config,
            provider_factory,
        }
    }
}

type ProviderFactory = Arc<
    dyn Fn(&LanguageModel, &RociConfig) -> Result<Box<dyn provider::ModelProvider>, RociError>
        + Send
        + Sync,
>;

mod control;
mod engine;
mod limits;
mod message_events;
mod tooling;

#[cfg(test)]
#[path = "runner/tests/mod.rs"]
mod tests;
