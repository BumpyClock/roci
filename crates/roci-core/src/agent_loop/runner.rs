//! Runner interfaces for the agent loop.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{future, StreamExt};
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::{self, Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::message::AgentMessage;
use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider::{self, ProviderRegistry, ProviderRequest, ToolDefinition};
use crate::tools::{tool::Tool, ToolUpdateCallback};
use crate::types::{
    message::ContentPart, AgentToolCall, AgentToolResult, GenerationSettings, ModelMessage,
    StreamEventType, TextStreamDelta,
};

use super::approvals::{
    ApprovalDecision, ApprovalHandler, ApprovalKind, ApprovalPolicy, ApprovalRequest,
};
use super::compaction::estimate_context_usage;
use super::events::{
    AgentEvent, RunEvent, RunEventPayload, RunEventStream, RunLifecycle, ToolUpdatePayload,
};
use super::types::{RunId, RunResult};

/// Callback used for streaming run events.
pub type RunEventSink = Arc<dyn Fn(RunEvent) + Send + Sync>;
/// Hook to compact/prune a message history before the next provider call.
pub type CompactionHandler = Arc<
    dyn Fn(
            Vec<ModelMessage>,
        )
            -> Pin<Box<dyn Future<Output = Result<Option<Vec<ModelMessage>>, RociError>> + Send>>
        + Send
        + Sync,
>;
/// Hook to redact/transform tool results before persistence or context assembly.
pub type ToolResultPersistHandler = Arc<dyn Fn(AgentToolResult) -> AgentToolResult + Send + Sync>;

/// Async callback to retrieve messages between loop phases.
pub type MessageBatchFn =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Vec<ModelMessage>> + Send>> + Send + Sync>;

/// Callback to retrieve steering messages between tool batches.
pub type SteeringMessagesFn = MessageBatchFn;

/// Callback to retrieve follow-up messages after the inner loop completes.
pub type FollowUpMessagesFn = MessageBatchFn;

/// Hook to transform the message context before each LLM call.
pub type TransformContextFn = Arc<
    dyn Fn(Vec<ModelMessage>) -> Pin<Box<dyn Future<Output = Vec<ModelMessage>> + Send>>
        + Send
        + Sync,
>;

/// Hook to convert/filter agent-level messages into provider-facing LLM messages.
///
/// This runs before `transform_context` and provider sanitization.
pub type ConvertToLlmFn = Arc<
    dyn Fn(Vec<AgentMessage>) -> Pin<Box<dyn Future<Output = Vec<ModelMessage>> + Send>>
        + Send
        + Sync,
>;

/// Sink for high-level AgentEvent emission (separate from RunEvent).
pub type AgentEventSink = Arc<dyn Fn(AgentEvent) + Send + Sync>;

#[derive(Clone, Default)]
pub struct RunHooks {
    pub compaction: Option<CompactionHandler>,
    pub tool_result_persist: Option<ToolResultPersistHandler>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoCompactionConfig {
    pub reserve_tokens: usize,
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
            get_steering_messages: None,
            get_follow_up_messages: None,
            transform_context: None,
            convert_to_llm: None,
            agent_event_sink: None,
            session_id: None,
            transport: None,
            max_retry_delay_ms: None,
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

const DEFAULT_MAX_ITERATIONS: usize = 20;
const DEFAULT_MAX_TOOL_FAILURES: usize = 8;
const DEFAULT_ITERATION_EXTENSION: usize = 20;
const DEFAULT_MAX_ITERATION_EXTENSIONS: usize = 3;
const RUNNER_MAX_ITERATIONS_ENV: &str = "HOMIE_ROCI_RUNNER_MAX_ITERATIONS";
const RUNNER_MAX_TOOL_FAILURES_ENV: &str = "HOMIE_ROCI_RUNNER_MAX_TOOL_FAILURES";
const RUNNER_ITERATION_EXTENSION_ENV: &str = "HOMIE_ROCI_RUNNER_ITERATION_EXTENSION";
const RUNNER_MAX_ITERATION_EXTENSIONS_ENV: &str = "HOMIE_ROCI_RUNNER_MAX_ITERATION_EXTENSIONS";
const RUNNER_MAX_ITERATIONS_KEYS: [&str; 3] = [
    "runner.max_iterations",
    "agent_loop.max_iterations",
    "max_iterations",
];
const RUNNER_MAX_TOOL_FAILURES_KEYS: [&str; 3] = [
    "runner.max_tool_failures",
    "agent_loop.max_tool_failures",
    "max_tool_failures",
];
const RUNNER_ITERATION_EXTENSION_KEYS: [&str; 3] = [
    "runner.iteration_extension",
    "agent_loop.iteration_extension",
    "iteration_extension",
];
const RUNNER_MAX_ITERATION_EXTENSIONS_KEYS: [&str; 3] = [
    "runner.max_iteration_extensions",
    "agent_loop.max_iteration_extensions",
    "max_iteration_extensions",
];
const PARALLEL_SAFE_TOOL_NAMES: [&str; 6] =
    ["read", "ls", "find", "grep", "web_search", "web_fetch"];

#[derive(Debug, Clone, Copy)]
struct RunnerLimits {
    max_iterations: usize,
    max_tool_failures: usize,
    iteration_extension: usize,
    max_iteration_extensions: usize,
}

impl RunnerLimits {
    fn from_request(request: &RunRequest) -> Self {
        Self {
            max_iterations: parse_runner_limit(
                &request.metadata,
                &RUNNER_MAX_ITERATIONS_KEYS,
                RUNNER_MAX_ITERATIONS_ENV,
                DEFAULT_MAX_ITERATIONS,
            ),
            max_tool_failures: parse_runner_limit(
                &request.metadata,
                &RUNNER_MAX_TOOL_FAILURES_KEYS,
                RUNNER_MAX_TOOL_FAILURES_ENV,
                DEFAULT_MAX_TOOL_FAILURES,
            ),
            iteration_extension: parse_runner_limit(
                &request.metadata,
                &RUNNER_ITERATION_EXTENSION_KEYS,
                RUNNER_ITERATION_EXTENSION_ENV,
                DEFAULT_ITERATION_EXTENSION,
            ),
            max_iteration_extensions: parse_runner_limit(
                &request.metadata,
                &RUNNER_MAX_ITERATION_EXTENSIONS_KEYS,
                RUNNER_MAX_ITERATION_EXTENSIONS_ENV,
                DEFAULT_MAX_ITERATION_EXTENSIONS,
            ),
        }
    }
}

fn parse_runner_limit(
    metadata: &HashMap<String, String>,
    keys: &[&str],
    env_key: &str,
    default: usize,
) -> usize {
    for key in keys {
        if let Some(value) = metadata.get(*key) {
            if let Some(parsed) = parse_positive_usize(value) {
                return parsed;
            }
        }
    }
    if let Ok(value) = std::env::var(env_key) {
        if let Some(parsed) = parse_positive_usize(&value) {
            return parsed;
        }
    }
    default
}

fn parse_positive_usize(value: &str) -> Option<usize> {
    let parsed = value.trim().parse::<usize>().ok()?;
    if parsed == 0 {
        None
    } else {
        Some(parsed)
    }
}

fn is_parallel_safe_tool(tool_name: &str) -> bool {
    PARALLEL_SAFE_TOOL_NAMES
        .iter()
        .any(|candidate| candidate == &tool_name)
}

fn approval_allows_execution(decision: ApprovalDecision) -> bool {
    matches!(
        decision,
        ApprovalDecision::Accept | ApprovalDecision::AcceptForSession
    )
}

fn declined_tool_result(call: &AgentToolCall) -> AgentToolResult {
    AgentToolResult {
        tool_call_id: call.id.clone(),
        result: serde_json::json!({ "error": "approval declined" }),
        is_error: true,
    }
}

fn canceled_tool_result(call: &AgentToolCall) -> AgentToolResult {
    AgentToolResult {
        tool_call_id: call.id.clone(),
        result: serde_json::json!({ "error": "canceled" }),
        is_error: true,
    }
}

fn emit_tool_execution_start(agent_emitter: &AgentEventEmitter, call: &AgentToolCall) {
    agent_emitter.emit(AgentEvent::ToolExecutionStart {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        args: call.arguments.clone(),
    });
}

fn emit_tool_execution_end(
    agent_emitter: &AgentEventEmitter,
    call: &AgentToolCall,
    result: &AgentToolResult,
) {
    agent_emitter.emit(AgentEvent::ToolExecutionEnd {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        result: result.clone(),
        is_error: result.is_error,
    });
}

fn build_assistant_message(iteration_text: &str, tool_calls: &[AgentToolCall]) -> ModelMessage {
    let mut content: Vec<ContentPart> = Vec::new();
    if !iteration_text.is_empty() {
        content.push(ContentPart::Text {
            text: iteration_text.to_string(),
        });
    }
    for call in tool_calls {
        content.push(ContentPart::ToolCall(call.clone()));
    }
    if content.is_empty() {
        content.push(ContentPart::Text {
            text: String::new(),
        });
    }
    ModelMessage {
        role: crate::types::Role::Assistant,
        content,
        name: None,
        timestamp: Some(chrono::Utc::now()),
    }
}

fn emit_message_start_if_needed(
    agent_emitter: &AgentEventEmitter,
    message_open: &mut bool,
    iteration_text: &str,
    tool_calls: &[AgentToolCall],
) {
    if !*message_open {
        agent_emitter.emit(AgentEvent::MessageStart {
            message: build_assistant_message(iteration_text, tool_calls),
        });
        *message_open = true;
    }
}

fn emit_message_end_if_open(
    agent_emitter: &AgentEventEmitter,
    message_open: &mut bool,
    iteration_text: &str,
    tool_calls: &[AgentToolCall],
) {
    if *message_open {
        agent_emitter.emit(AgentEvent::MessageEnd {
            message: build_assistant_message(iteration_text, tool_calls),
        });
        *message_open = false;
    }
}

fn emit_message_lifecycle(agent_emitter: &AgentEventEmitter, message: &ModelMessage) {
    agent_emitter.emit(AgentEvent::MessageStart {
        message: message.clone(),
    });
    agent_emitter.emit(AgentEvent::MessageEnd {
        message: message.clone(),
    });
}

async fn execute_tool_call(
    tools: &[Arc<dyn Tool>],
    call: &AgentToolCall,
    agent_emitter: &AgentEventEmitter,
    cancel: CancellationToken,
) -> AgentToolResult {
    let tool = tools.iter().find(|t| t.name() == call.name);
    match tool {
        Some(tool) => {
            let schema = &tool.parameters().schema;
            if let Err(validation_error) =
                crate::tools::validation::validate_arguments(&call.arguments, schema)
            {
                return AgentToolResult {
                    tool_call_id: call.id.clone(),
                    result: serde_json::json!({
                        "error": format!("Argument validation failed: {}", validation_error)
                    }),
                    is_error: true,
                };
            }
            let args = crate::tools::arguments::ToolArguments::new(call.arguments.clone());
            let ctx = crate::tools::tool::ToolExecutionContext {
                metadata: serde_json::Value::Null,
                tool_call_id: Some(call.id.clone()),
                tool_name: Some(call.name.clone()),
            };
            let call_id = call.id.clone();
            let call_name = call.name.clone();
            let call_args = call.arguments.clone();
            let update_emitter = agent_emitter.clone();
            let on_update: ToolUpdateCallback =
                Arc::new(move |partial_result: ToolUpdatePayload| {
                    update_emitter.emit(AgentEvent::ToolExecutionUpdate {
                        tool_call_id: call_id.clone(),
                        tool_name: call_name.clone(),
                        args: call_args.clone(),
                        partial_result,
                    });
                });
            match tool.execute_ext(&args, &ctx, cancel, Some(on_update)).await {
                Ok(val) => AgentToolResult {
                    tool_call_id: call.id.clone(),
                    result: val,
                    is_error: false,
                },
                Err(error) => AgentToolResult {
                    tool_call_id: call.id.clone(),
                    result: serde_json::json!({ "error": error.to_string() }),
                    is_error: true,
                },
            }
        }
        None => AgentToolResult {
            tool_call_id: call.id.clone(),
            result: serde_json::json!({ "error": format!("Tool '{}' not found", call.name) }),
            is_error: true,
        },
    }
}

async fn execute_parallel_tool_calls(
    tools: &[Arc<dyn Tool>],
    calls: &[AgentToolCall],
    agent_emitter: &AgentEventEmitter,
    cancel: CancellationToken,
) -> Vec<AgentToolResult> {
    let futures = calls
        .iter()
        .map(|call| execute_tool_call(tools, call, agent_emitter, cancel.child_token()));
    future::join_all(futures).await
}

fn append_tool_result(
    hooks: &RunHooks,
    emitter: &RunEventEmitter,
    agent_emitter: &AgentEventEmitter,
    call: &AgentToolCall,
    mut result: AgentToolResult,
    iteration_failures: &mut usize,
    messages: &mut Vec<ModelMessage>,
) {
    if let Some(handler) = hooks.tool_result_persist.as_ref() {
        result = handler(result);
    }

    if result.is_error {
        *iteration_failures = iteration_failures.saturating_add(1);
    }

    emitter.emit(
        RunEventStream::Tool,
        RunEventPayload::ToolResult {
            result: result.clone(),
        },
    );
    emitter.emit(
        RunEventStream::Tool,
        RunEventPayload::ToolCallCompleted { call: call.clone() },
    );

    let tool_result_message =
        ModelMessage::tool_result(result.tool_call_id.clone(), result.result, result.is_error);
    emit_message_lifecycle(agent_emitter, &tool_result_message);
    messages.push(tool_result_message);
}

fn append_skipped_tool_call(
    hooks: &RunHooks,
    emitter: &RunEventEmitter,
    agent_emitter: &AgentEventEmitter,
    call: &AgentToolCall,
    iteration_failures: &mut usize,
    messages: &mut Vec<ModelMessage>,
) -> AgentToolResult {
    let skipped_result = AgentToolResult {
        tool_call_id: call.id.clone(),
        result: serde_json::json!({ "error": "Skipped due to steering message" }),
        is_error: true,
    };
    emit_tool_execution_start(agent_emitter, call);
    emit_tool_execution_end(agent_emitter, call, &skipped_result);
    append_tool_result(
        hooks,
        emitter,
        agent_emitter,
        call,
        skipped_result.clone(),
        iteration_failures,
        messages,
    );
    skipped_result
}

#[async_trait]
impl Runner for LoopRunner {
    async fn start(&self, request: RunRequest) -> Result<RunHandle, RociError> {
        let (handle, mut abort_rx, result_tx, mut input_rx) = RunHandle::new(request.run_id);
        let config = self.config.clone();
        let provider_factory = self.provider_factory.clone();

        tokio::spawn(async move {
            if debug_enabled() {
                tracing::debug!(
                    run_id = %request.run_id,
                    model = %request.model.to_string(),
                    "roci run start"
                );
            }
            let limits = RunnerLimits::from_request(&request);
            let emitter = RunEventEmitter::new(request.run_id, request.event_sink);
            let agent_emitter = AgentEventEmitter::new(request.agent_event_sink.clone());
            emitter.emit(
                RunEventStream::Lifecycle,
                RunEventPayload::Lifecycle {
                    state: RunLifecycle::Started,
                },
            );
            agent_emitter.emit(AgentEvent::AgentStart {
                run_id: request.run_id,
            });

            if debug_enabled() {
                tracing::debug!(
                    run_id = %request.run_id,
                    max_iterations = limits.max_iterations,
                    max_tool_failures = limits.max_tool_failures,
                    iteration_extension = limits.iteration_extension,
                    max_iteration_extensions = limits.max_iteration_extensions,
                    "roci runner limits"
                );
            }

            let mut messages = request.messages.clone();
            for message in &messages {
                emit_message_lifecycle(&agent_emitter, message);
            }

            if let Err(err) = provider::validate_transport_preference(request.transport.as_deref())
            {
                agent_emitter.emit(AgentEvent::AgentEnd {
                    run_id: request.run_id,
                });
                let _ = result_tx.send(emit_failed_result(&emitter, err.to_string(), &messages));
                return;
            }

            let provider = match provider_factory(&request.model, &config) {
                Ok(provider) => provider,
                Err(err) => {
                    agent_emitter.emit(AgentEvent::AgentEnd {
                        run_id: request.run_id,
                    });
                    let _ =
                        result_tx.send(emit_failed_result(&emitter, err.to_string(), &messages));
                    return;
                }
            };

            let tool_defs: Option<Vec<ToolDefinition>> = if request.tools.is_empty() {
                None
            } else {
                Some(
                    request
                        .tools
                        .iter()
                        .map(|t| ToolDefinition {
                            name: t.name().to_string(),
                            description: t.description().to_string(),
                            parameters: t.parameters().schema.clone(),
                        })
                        .collect(),
                )
            };

            let mut iteration = 0usize;
            let mut consecutive_failed_iterations = 0usize;
            let mut max_iterations = limits.max_iterations;
            let mut iteration_extensions_used = 0usize;
            let mut turn_index = 0usize;
            let run_cancel_token = CancellationToken::new();

            'outer: loop {
                'inner: loop {
                    iteration += 1;
                    turn_index += 1;
                    agent_emitter.emit(AgentEvent::TurnStart {
                        run_id: request.run_id,
                        turn_index,
                    });

                    if iteration > max_iterations {
                        if iteration_extensions_used >= limits.max_iteration_extensions {
                            let reason = format!(
                            "tool loop exceeded max iterations (max_iterations={}, extensions_used={})",
                            max_iterations, iteration_extensions_used
                        );
                            agent_emitter.emit(AgentEvent::AgentEnd {
                                run_id: request.run_id,
                            });
                            let _ = result_tx.send(emit_failed_result(&emitter, reason, &messages));
                            return;
                        }

                        let decision = resolve_iteration_limit_approval(
                            &emitter,
                            request.approval_handler.as_ref(),
                            request.run_id,
                            iteration,
                            max_iterations,
                            limits.iteration_extension,
                            iteration_extensions_used + 1,
                        )
                        .await;

                        match decision {
                            ApprovalDecision::Accept | ApprovalDecision::AcceptForSession => {
                                max_iterations =
                                    max_iterations.saturating_add(limits.iteration_extension);
                                iteration_extensions_used =
                                    iteration_extensions_used.saturating_add(1);
                                if debug_enabled() {
                                    tracing::debug!(
                                        run_id = %request.run_id,
                                        iteration,
                                        max_iterations,
                                        iteration_extensions_used,
                                        "roci iteration limit extended"
                                    );
                                }
                            }
                            ApprovalDecision::Cancel => {
                                emitter.emit(
                                    RunEventStream::Lifecycle,
                                    RunEventPayload::Lifecycle {
                                        state: RunLifecycle::Canceled,
                                    },
                                );
                                agent_emitter.emit(AgentEvent::AgentEnd {
                                    run_id: request.run_id,
                                });
                                let _ = result_tx
                                    .send(RunResult::canceled_with_messages(messages.clone()));
                                return;
                            }
                            ApprovalDecision::Decline => {
                                let reason = format!(
                                "tool loop exceeded max iterations (max_iterations={max_iterations}); continuation declined"
                            );
                                agent_emitter.emit(AgentEvent::AgentEnd {
                                    run_id: request.run_id,
                                });
                                let _ =
                                    result_tx.send(emit_failed_result(&emitter, reason, &messages));
                                return;
                            }
                        }
                    }

                    while let Ok(message) = input_rx.try_recv() {
                        emit_message_lifecycle(&agent_emitter, &message);
                        messages.push(message);
                    }

                    // Inject steering messages before LLM call
                    if let Some(ref get_steering) = request.get_steering_messages {
                        for msg in get_steering().await {
                            emit_message_lifecycle(&agent_emitter, &msg);
                            messages.push(msg);
                        }
                    }

                    let should_compact = request.auto_compaction.as_ref().is_some_and(|config| {
                        let usage = estimate_context_usage(
                            &messages,
                            provider.capabilities().context_length,
                        );
                        usage.used_tokens
                            > usage.context_window.saturating_sub(config.reserve_tokens)
                    });
                    if should_compact {
                        let Some(compact) = request.hooks.compaction.as_ref() else {
                            agent_emitter.emit(AgentEvent::AgentEnd {
                                run_id: request.run_id,
                            });
                            let _ = result_tx.send(emit_failed_result(
                                &emitter,
                                "auto-compaction is enabled but no compaction hook is configured",
                                &messages,
                            ));
                            return;
                        };
                        match compact(messages.clone()).await {
                            Ok(Some(compacted)) => {
                                messages = compacted;
                            }
                            Ok(None) => {}
                            Err(err) => {
                                agent_emitter.emit(AgentEvent::AgentEnd {
                                    run_id: request.run_id,
                                });
                                let _ = result_tx.send(emit_failed_result(
                                    &emitter,
                                    format!("compaction failed: {err}"),
                                    &messages,
                                ));
                                return;
                            }
                        }
                    }

                    let llm_context = if let Some(ref convert) = request.convert_to_llm {
                        let agent_messages: Vec<AgentMessage> = messages
                            .iter()
                            .cloned()
                            .map(AgentMessage::from_model)
                            .collect();
                        convert(agent_messages).await
                    } else {
                        messages.clone()
                    };
                    let transformed = if let Some(ref transform) = request.transform_context {
                        transform(llm_context).await
                    } else {
                        llm_context
                    };
                    let provider_messages = provider::sanitize_messages_for_provider(
                        &transformed,
                        provider.provider_name(),
                    );
                    let req = ProviderRequest {
                        messages: provider_messages,
                        settings: request.settings.clone(),
                        tools: tool_defs.clone(),
                        response_format: request.settings.response_format.clone(),
                        session_id: request.session_id.clone(),
                        transport: request.transport.clone(),
                    };

                    let mut stream = loop {
                        match provider.stream_text(&req).await {
                            Ok(stream) => break stream,
                            Err(RociError::RateLimited { retry_after_ms }) => {
                                let retry_after_ms = retry_after_ms.unwrap_or(0);
                                if retry_after_ms == 0 {
                                    agent_emitter.emit(AgentEvent::AgentEnd {
                                        run_id: request.run_id,
                                    });
                                    let _ = result_tx.send(emit_failed_result(
                                        &emitter,
                                        "rate limited without retry_after hint",
                                        &messages,
                                    ));
                                    return;
                                }
                                if let Some(max_retry_delay_ms) = request.max_retry_delay_ms {
                                    if max_retry_delay_ms > 0 && retry_after_ms > max_retry_delay_ms
                                    {
                                        agent_emitter.emit(AgentEvent::AgentEnd {
                                            run_id: request.run_id,
                                        });
                                        let _ = result_tx.send(emit_failed_result(
                                            &emitter,
                                            format!(
                                                "rate limit retry delay {retry_after_ms}ms exceeds max_retry_delay_ms={max_retry_delay_ms}"
                                            ),
                                            &messages,
                                        ));
                                        return;
                                    }
                                }
                                tokio::select! {
                                    _ = &mut abort_rx => {
                                        run_cancel_token.cancel();
                                        emitter.emit(
                                            RunEventStream::Lifecycle,
                                            RunEventPayload::Lifecycle {
                                                state: RunLifecycle::Canceled,
                                            },
                                        );
                                        agent_emitter.emit(AgentEvent::AgentEnd { run_id: request.run_id });
                                        let _ = result_tx
                                            .send(RunResult::canceled_with_messages(messages.clone()));
                                        return;
                                    }
                                    _ = time::sleep(Duration::from_millis(retry_after_ms)) => {}
                                }
                            }
                            Err(err) => {
                                agent_emitter.emit(AgentEvent::AgentEnd {
                                    run_id: request.run_id,
                                });
                                let _ = result_tx.send(emit_failed_result(
                                    &emitter,
                                    err.to_string(),
                                    &messages,
                                ));
                                return;
                            }
                        }
                    };

                    let mut iteration_text = String::new();
                    let mut tool_calls: Vec<AgentToolCall> = Vec::new();
                    let mut stream_done = false;
                    let mut message_open = false;
                    let idle_timeout_ms =
                        request.settings.stream_idle_timeout_ms.unwrap_or(120_000);
                    let mut idle_sleep = (idle_timeout_ms > 0)
                        .then(|| Box::pin(time::sleep(Duration::from_millis(idle_timeout_ms))));
                    loop {
                        if let Some(ref mut sleep) = idle_sleep {
                            tokio::select! {
                                _ = &mut abort_rx => {
                                    run_cancel_token.cancel();
                                    emit_message_end_if_open(
                                        &agent_emitter,
                                        &mut message_open,
                                        &iteration_text,
                                        &tool_calls,
                                    );
                                    emitter.emit(
                                        RunEventStream::Lifecycle,
                                        RunEventPayload::Lifecycle {
                                            state: RunLifecycle::Canceled,
                                        },
                                    );
                                    agent_emitter.emit(AgentEvent::AgentEnd { run_id: request.run_id });
                                    let _ = result_tx
                                        .send(RunResult::canceled_with_messages(messages.clone()));
                                    return;
                                }
                                _ = sleep.as_mut() => {
                                    emit_message_end_if_open(
                                        &agent_emitter,
                                        &mut message_open,
                                        &iteration_text,
                                        &tool_calls,
                                    );
                                    agent_emitter.emit(AgentEvent::AgentEnd { run_id: request.run_id });
                                    let _ = result_tx.send(emit_failed_result(
                                        &emitter,
                                        "stream idle timeout",
                                        &messages,
                                    ));
                                    return;
                                }
                                delta = stream.next() => {
                                    let Some(delta) = delta else { break; };
                                    match delta {
                                        Ok(delta) => {
                                            sleep.as_mut().reset(
                                                time::Instant::now() + Duration::from_millis(idle_timeout_ms),
                                            );
                                            if let Some(reason) = process_stream_delta(
                                                &emitter,
                                                &agent_emitter,
                                                delta,
                                                &mut iteration_text,
                                                &mut tool_calls,
                                                &mut stream_done,
                                                &mut message_open,
                                            ) {
                                                emit_message_end_if_open(
                                                    &agent_emitter,
                                                    &mut message_open,
                                                    &iteration_text,
                                                    &tool_calls,
                                                );
                                                agent_emitter.emit(AgentEvent::AgentEnd { run_id: request.run_id });
                                                let _ = result_tx.send(emit_failed_result(
                                                    &emitter,
                                                    reason,
                                                    &messages,
                                                ));
                                                return;
                                            }
                                            if stream_done {
                                                break;
                                            }
                                        }
                                        Err(err) => {
                                            emit_message_end_if_open(
                                                &agent_emitter,
                                                &mut message_open,
                                                &iteration_text,
                                                &tool_calls,
                                            );
                                            agent_emitter.emit(AgentEvent::AgentEnd { run_id: request.run_id });
                                            let _ = result_tx.send(emit_failed_result(
                                                &emitter,
                                                err.to_string(),
                                                &messages,
                                            ));
                                            return;
                                        }
                                    }
                                }
                            }
                        } else {
                            tokio::select! {
                                _ = &mut abort_rx => {
                                    run_cancel_token.cancel();
                                    emit_message_end_if_open(
                                        &agent_emitter,
                                        &mut message_open,
                                        &iteration_text,
                                        &tool_calls,
                                    );
                                    emitter.emit(
                                        RunEventStream::Lifecycle,
                                        RunEventPayload::Lifecycle {
                                            state: RunLifecycle::Canceled,
                                        },
                                    );
                                    agent_emitter.emit(AgentEvent::AgentEnd { run_id: request.run_id });
                                    let _ = result_tx
                                        .send(RunResult::canceled_with_messages(messages.clone()));
                                    return;
                                }
                                delta = stream.next() => {
                                    let Some(delta) = delta else { break; };
                                    match delta {
                                        Ok(delta) => {
                                            if let Some(reason) = process_stream_delta(
                                                &emitter,
                                                &agent_emitter,
                                                delta,
                                                &mut iteration_text,
                                                &mut tool_calls,
                                                &mut stream_done,
                                                &mut message_open,
                                            ) {
                                                emit_message_end_if_open(
                                                    &agent_emitter,
                                                    &mut message_open,
                                                    &iteration_text,
                                                    &tool_calls,
                                                );
                                                agent_emitter.emit(AgentEvent::AgentEnd { run_id: request.run_id });
                                                let _ = result_tx.send(emit_failed_result(
                                                    &emitter,
                                                    reason,
                                                    &messages,
                                                ));
                                                return;
                                            }
                                            if stream_done {
                                                break;
                                            }
                                        }
                                        Err(err) => {
                                            emit_message_end_if_open(
                                                &agent_emitter,
                                                &mut message_open,
                                                &iteration_text,
                                                &tool_calls,
                                            );
                                            agent_emitter.emit(AgentEvent::AgentEnd { run_id: request.run_id });
                                            let _ = result_tx.send(emit_failed_result(
                                                &emitter,
                                                err.to_string(),
                                                &messages,
                                            ));
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    emit_message_end_if_open(
                        &agent_emitter,
                        &mut message_open,
                        &iteration_text,
                        &tool_calls,
                    );

                    if debug_enabled() {
                        let tool_names = tool_calls
                            .iter()
                            .map(|call| call.name.as_str())
                            .collect::<Vec<_>>()
                            .join(",");
                        tracing::debug!(
                            run_id = %request.run_id,
                            iteration,
                            stream_done,
                            tool_calls = tool_calls.len(),
                            tool_names = %tool_names,
                            text_len = iteration_text.len(),
                            "roci iteration complete"
                        );
                    }

                    if tool_calls.is_empty() {
                        agent_emitter.emit(AgentEvent::TurnEnd {
                            run_id: request.run_id,
                            turn_index,
                            tool_results: vec![],
                        });
                        break 'inner;
                    }

                    let mut assistant_content: Vec<ContentPart> = Vec::new();
                    if !iteration_text.is_empty() {
                        assistant_content.push(ContentPart::Text {
                            text: iteration_text,
                        });
                    }
                    for call in &tool_calls {
                        assistant_content.push(ContentPart::ToolCall(call.clone()));
                    }
                    messages.push(ModelMessage {
                        role: crate::types::Role::Assistant,
                        content: assistant_content,
                        name: None,
                        timestamp: Some(chrono::Utc::now()),
                    });

                    let mut iteration_failures = 0usize;
                    let mut turn_tool_results: Vec<AgentToolResult> = Vec::new();
                    let mut steering_interrupted = false;
                    let mut pending_parallel_calls: Vec<AgentToolCall> = Vec::new();
                    for (call_idx, call) in tool_calls.iter().enumerate() {
                        let decision = resolve_approval(
                            &emitter,
                            &request.approval_policy,
                            request.approval_handler.as_ref(),
                            call,
                        )
                        .await;

                        if matches!(decision, ApprovalDecision::Cancel) {
                            run_cancel_token.cancel();
                            emitter.emit(
                                RunEventStream::Lifecycle,
                                RunEventPayload::Lifecycle {
                                    state: RunLifecycle::Canceled,
                                },
                            );
                            agent_emitter.emit(AgentEvent::AgentEnd {
                                run_id: request.run_id,
                            });
                            let _ =
                                result_tx.send(RunResult::canceled_with_messages(messages.clone()));
                            if debug_enabled() {
                                tracing::debug!(run_id = %request.run_id, "roci run canceled");
                            }
                            return;
                        }

                        let can_execute = approval_allows_execution(decision);
                        if can_execute && is_parallel_safe_tool(&call.name) {
                            pending_parallel_calls.push(call.clone());
                            continue;
                        }

                        if !pending_parallel_calls.is_empty() {
                            for parallel_call in &pending_parallel_calls {
                                emit_tool_execution_start(&agent_emitter, parallel_call);
                            }
                            let parallel_results = tokio::select! {
                                _ = &mut abort_rx => {
                                    run_cancel_token.cancel();
                                    for parallel_call in &pending_parallel_calls {
                                        let canceled_result = canceled_tool_result(parallel_call);
                                        emit_tool_execution_end(&agent_emitter, parallel_call, &canceled_result);
                                    }
                                    emitter.emit(
                                        RunEventStream::Lifecycle,
                                        RunEventPayload::Lifecycle {
                                            state: RunLifecycle::Canceled,
                                        },
                                    );
                                    agent_emitter.emit(AgentEvent::AgentEnd { run_id: request.run_id });
                                    let _ = result_tx
                                        .send(RunResult::canceled_with_messages(messages.clone()));
                                    if debug_enabled() {
                                        tracing::debug!(run_id = %request.run_id, "roci run canceled");
                                    }
                                    return;
                                }
                                results = execute_parallel_tool_calls(
                                    &request.tools,
                                    &pending_parallel_calls,
                                    &agent_emitter,
                                    run_cancel_token.child_token(),
                                ) => results,
                            };
                            for (parallel_call, parallel_result) in pending_parallel_calls
                                .drain(..)
                                .zip(parallel_results.into_iter())
                            {
                                emit_tool_execution_end(
                                    &agent_emitter,
                                    &parallel_call,
                                    &parallel_result,
                                );
                                turn_tool_results.push(parallel_result.clone());
                                append_tool_result(
                                    &request.hooks,
                                    &emitter,
                                    &agent_emitter,
                                    &parallel_call,
                                    parallel_result,
                                    &mut iteration_failures,
                                    &mut messages,
                                );
                            }

                            // Check for steering after parallel batch flush
                            if let Some(ref get_steering) = request.get_steering_messages {
                                let steering = get_steering().await;
                                if !steering.is_empty() {
                                    for remaining_call in &tool_calls[call_idx + 1..] {
                                        let skipped = append_skipped_tool_call(
                                            &request.hooks,
                                            &emitter,
                                            &agent_emitter,
                                            remaining_call,
                                            &mut iteration_failures,
                                            &mut messages,
                                        );
                                        turn_tool_results.push(skipped);
                                    }
                                    for msg in steering {
                                        emit_message_lifecycle(&agent_emitter, &msg);
                                        messages.push(msg);
                                    }
                                    steering_interrupted = true;
                                    break;
                                }
                            }
                        }

                        let result = if can_execute {
                            emit_tool_execution_start(&agent_emitter, call);
                            tokio::select! {
                                _ = &mut abort_rx => {
                                    run_cancel_token.cancel();
                                    let canceled_result = canceled_tool_result(call);
                                    emit_tool_execution_end(&agent_emitter, call, &canceled_result);
                                    emitter.emit(
                                        RunEventStream::Lifecycle,
                                        RunEventPayload::Lifecycle {
                                            state: RunLifecycle::Canceled,
                                        },
                                    );
                                    agent_emitter.emit(AgentEvent::AgentEnd { run_id: request.run_id });
                                    let _ = result_tx
                                        .send(RunResult::canceled_with_messages(messages.clone()));
                                    if debug_enabled() {
                                        tracing::debug!(run_id = %request.run_id, "roci run canceled");
                                    }
                                    return;
                                }
                                result = execute_tool_call(
                                    &request.tools,
                                    call,
                                    &agent_emitter,
                                    run_cancel_token.child_token(),
                                ) => result,
                            }
                        } else {
                            declined_tool_result(call)
                        };
                        if can_execute {
                            emit_tool_execution_end(&agent_emitter, call, &result);
                        }

                        turn_tool_results.push(result.clone());
                        append_tool_result(
                            &request.hooks,
                            &emitter,
                            &agent_emitter,
                            call,
                            result,
                            &mut iteration_failures,
                            &mut messages,
                        );

                        // Check for steering after sequential tool execution
                        if let Some(ref get_steering) = request.get_steering_messages {
                            let steering = get_steering().await;
                            if !steering.is_empty() {
                                for remaining_call in &tool_calls[call_idx + 1..] {
                                    let skipped = append_skipped_tool_call(
                                        &request.hooks,
                                        &emitter,
                                        &agent_emitter,
                                        remaining_call,
                                        &mut iteration_failures,
                                        &mut messages,
                                    );
                                    turn_tool_results.push(skipped);
                                }
                                for msg in steering {
                                    emit_message_lifecycle(&agent_emitter, &msg);
                                    messages.push(msg);
                                }
                                steering_interrupted = true;
                                break;
                            }
                        }
                    }

                    if steering_interrupted {
                        agent_emitter.emit(AgentEvent::TurnEnd {
                            run_id: request.run_id,
                            turn_index,
                            tool_results: turn_tool_results,
                        });
                        continue 'inner;
                    }

                    if !pending_parallel_calls.is_empty() {
                        for parallel_call in &pending_parallel_calls {
                            emit_tool_execution_start(&agent_emitter, parallel_call);
                        }
                        let parallel_results = tokio::select! {
                            _ = &mut abort_rx => {
                                run_cancel_token.cancel();
                                for parallel_call in &pending_parallel_calls {
                                    let canceled_result = canceled_tool_result(parallel_call);
                                    emit_tool_execution_end(&agent_emitter, parallel_call, &canceled_result);
                                }
                                emitter.emit(
                                    RunEventStream::Lifecycle,
                                    RunEventPayload::Lifecycle {
                                        state: RunLifecycle::Canceled,
                                    },
                                );
                                agent_emitter.emit(AgentEvent::AgentEnd { run_id: request.run_id });
                                let _ = result_tx
                                    .send(RunResult::canceled_with_messages(messages.clone()));
                                if debug_enabled() {
                                    tracing::debug!(run_id = %request.run_id, "roci run canceled");
                                }
                                return;
                            }
                            results = execute_parallel_tool_calls(
                                &request.tools,
                                &pending_parallel_calls,
                                &agent_emitter,
                                run_cancel_token.child_token(),
                            ) => results,
                        };
                        for (parallel_call, parallel_result) in pending_parallel_calls
                            .drain(..)
                            .zip(parallel_results.into_iter())
                        {
                            emit_tool_execution_end(
                                &agent_emitter,
                                &parallel_call,
                                &parallel_result,
                            );
                            turn_tool_results.push(parallel_result.clone());
                            append_tool_result(
                                &request.hooks,
                                &emitter,
                                &agent_emitter,
                                &parallel_call,
                                parallel_result,
                                &mut iteration_failures,
                                &mut messages,
                            );
                        }
                    }

                    agent_emitter.emit(AgentEvent::TurnEnd {
                        run_id: request.run_id,
                        turn_index,
                        tool_results: turn_tool_results,
                    });

                    if iteration_failures == tool_calls.len() {
                        consecutive_failed_iterations =
                            consecutive_failed_iterations.saturating_add(1);
                    } else {
                        consecutive_failed_iterations = 0;
                    }

                    if consecutive_failed_iterations >= limits.max_tool_failures {
                        let reason = format!(
                        "tool call failure limit reached (max_failures={}, consecutive_failures={})",
                        limits.max_tool_failures,
                        consecutive_failed_iterations
                    );
                        agent_emitter.emit(AgentEvent::AgentEnd {
                            run_id: request.run_id,
                        });
                        let _ = result_tx.send(emit_failed_result(&emitter, reason, &messages));
                        return;
                    }
                } // end 'inner

                // Check for follow-up messages after the inner loop completes
                if let Some(ref get_follow_ups) = request.get_follow_up_messages {
                    let follow_ups = get_follow_ups().await;
                    if !follow_ups.is_empty() {
                        for msg in follow_ups {
                            emit_message_lifecycle(&agent_emitter, &msg);
                            messages.push(msg);
                        }
                        continue 'outer;
                    }
                }

                // No follow-ups — complete
                emitter.emit(
                    RunEventStream::Lifecycle,
                    RunEventPayload::Lifecycle {
                        state: RunLifecycle::Completed,
                    },
                );
                agent_emitter.emit(AgentEvent::AgentEnd {
                    run_id: request.run_id,
                });
                let _ = result_tx.send(RunResult::completed_with_messages(messages));
                if debug_enabled() {
                    tracing::debug!(run_id = %request.run_id, "roci run completed");
                }
                return;
            } // end 'outer
        });

        Ok(handle)
    }
}

fn emit_failed_result(
    emitter: &RunEventEmitter,
    reason: impl Into<String>,
    messages: &[ModelMessage],
) -> RunResult {
    let reason = reason.into();
    emitter.emit(
        RunEventStream::Lifecycle,
        RunEventPayload::Lifecycle {
            state: RunLifecycle::Failed {
                error: reason.clone(),
            },
        },
    );
    RunResult::failed_with_messages(reason, messages.to_vec())
}

fn process_stream_delta(
    emitter: &RunEventEmitter,
    agent_emitter: &AgentEventEmitter,
    delta: TextStreamDelta,
    iteration_text: &mut String,
    tool_calls: &mut Vec<AgentToolCall>,
    stream_done: &mut bool,
    message_open: &mut bool,
) -> Option<String> {
    let assistant_event = delta.clone();
    match delta.event_type {
        StreamEventType::ToolCallDelta => {
            if let Some(tc) = delta.tool_call {
                if tc.id.trim().is_empty() || tc.name.trim().is_empty() {
                    emitter.emit(
                        RunEventStream::System,
                        RunEventPayload::Error {
                            message: "stream tool_call_delta missing id/name".to_string(),
                        },
                    );
                    return None;
                }

                emit_message_start_if_needed(
                    agent_emitter,
                    message_open,
                    iteration_text,
                    tool_calls,
                );
                if let Some(existing) = tool_calls.iter_mut().find(|call| call.id == tc.id) {
                    *existing = tc.clone();
                    emitter.emit(
                        RunEventStream::Tool,
                        RunEventPayload::ToolCallDelta {
                            call_id: tc.id.clone(),
                            delta: tc.arguments.clone(),
                        },
                    );
                } else {
                    tool_calls.push(tc.clone());
                    emitter.emit(
                        RunEventStream::Tool,
                        RunEventPayload::ToolCallStarted { call: tc },
                    );
                }
                agent_emitter.emit(AgentEvent::MessageUpdate {
                    message: build_assistant_message(iteration_text, tool_calls),
                    assistant_message_event: assistant_event,
                });
            } else {
                emitter.emit(
                    RunEventStream::System,
                    RunEventPayload::Error {
                        message: "stream tool_call_delta missing tool_call payload".to_string(),
                    },
                );
            }
        }
        StreamEventType::Reasoning => {
            if let Some(reasoning) = delta.reasoning {
                if !reasoning.is_empty() {
                    emitter.emit(
                        RunEventStream::Reasoning,
                        RunEventPayload::ReasoningDelta {
                            text: reasoning.clone(),
                        },
                    );
                    emit_message_start_if_needed(
                        agent_emitter,
                        message_open,
                        iteration_text,
                        tool_calls,
                    );
                    agent_emitter.emit(AgentEvent::MessageUpdate {
                        message: build_assistant_message(iteration_text, tool_calls),
                        assistant_message_event: assistant_event,
                    });
                    agent_emitter.emit(AgentEvent::Reasoning { text: reasoning });
                }
            }
        }
        StreamEventType::TextDelta => {
            if !delta.text.is_empty() {
                iteration_text.push_str(&delta.text);
                emit_message_start_if_needed(
                    agent_emitter,
                    message_open,
                    iteration_text,
                    tool_calls,
                );
                emitter.emit(
                    RunEventStream::Assistant,
                    RunEventPayload::AssistantDelta {
                        text: delta.text.clone(),
                    },
                );
                agent_emitter.emit(AgentEvent::MessageUpdate {
                    message: build_assistant_message(iteration_text, tool_calls),
                    assistant_message_event: assistant_event,
                });
            }
        }
        StreamEventType::Error => {
            let message = if delta.text.trim().is_empty() {
                "stream error".to_string()
            } else {
                delta.text
            };
            emit_message_end_if_open(agent_emitter, message_open, iteration_text, tool_calls);
            return Some(message);
        }
        StreamEventType::Done => {
            *stream_done = true;
            emit_message_end_if_open(agent_emitter, message_open, iteration_text, tool_calls);
        }
        _ => {}
    }
    None
}

struct RunEventEmitter {
    run_id: RunId,
    seq: std::sync::atomic::AtomicU64,
    sink: Option<RunEventSink>,
}

impl RunEventEmitter {
    fn new(run_id: RunId, sink: Option<RunEventSink>) -> Self {
        Self {
            run_id,
            seq: std::sync::atomic::AtomicU64::new(1),
            sink,
        }
    }

    fn emit(&self, stream: RunEventStream, payload: RunEventPayload) {
        let Some(sink) = &self.sink else {
            return;
        };
        let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        (sink)(RunEvent {
            run_id: self.run_id,
            seq,
            timestamp: chrono::Utc::now(),
            stream,
            payload,
        });
    }
}

#[derive(Clone)]
struct AgentEventEmitter {
    sink: Option<AgentEventSink>,
}

impl AgentEventEmitter {
    fn new(sink: Option<AgentEventSink>) -> Self {
        Self { sink }
    }

    fn emit(&self, event: AgentEvent) {
        if let Some(sink) = &self.sink {
            (sink)(event);
        }
    }
}

async fn resolve_approval(
    emitter: &RunEventEmitter,
    policy: &ApprovalPolicy,
    handler: Option<&ApprovalHandler>,
    call: &AgentToolCall,
) -> ApprovalDecision {
    match policy {
        ApprovalPolicy::Always => ApprovalDecision::Accept,
        ApprovalPolicy::Never => ApprovalDecision::Decline,
        ApprovalPolicy::Ask => {
            let kind = approval_kind_for_tool(call);
            if matches!(kind, ApprovalKind::Other) {
                return ApprovalDecision::Accept;
            }
            let request = ApprovalRequest {
                id: call.id.clone(),
                kind,
                reason: Some(format!("Tool: {}", call.name)),
                payload: serde_json::json!({
                    "tool_name": call.name.clone(),
                    "tool_call_id": call.id.clone(),
                    "arguments": call.arguments.clone(),
                }),
                suggested_policy_change: None,
            };
            emitter.emit(
                RunEventStream::Approval,
                RunEventPayload::ApprovalRequired {
                    request: request.clone(),
                },
            );
            let Some(handler) = handler else {
                return ApprovalDecision::Decline;
            };
            handler(request).await
        }
    }
}

async fn resolve_iteration_limit_approval(
    emitter: &RunEventEmitter,
    handler: Option<&ApprovalHandler>,
    run_id: RunId,
    iteration: usize,
    current_limit: usize,
    extension: usize,
    attempt: usize,
) -> ApprovalDecision {
    let request = ApprovalRequest {
        id: format!("run-{run_id}-continue-{attempt}"),
        kind: ApprovalKind::Other,
        reason: Some(format!(
            "Reached iteration limit ({current_limit}). Continue for {extension} more iterations?"
        )),
        payload: serde_json::json!({
            "type": "iteration_limit",
            "run_id": run_id.to_string(),
            "iteration": iteration,
            "current_limit": current_limit,
            "extension": extension,
            "attempt": attempt,
        }),
        suggested_policy_change: None,
    };
    emitter.emit(
        RunEventStream::Approval,
        RunEventPayload::ApprovalRequired {
            request: request.clone(),
        },
    );
    let Some(handler) = handler else {
        return ApprovalDecision::Decline;
    };
    handler(request).await
}

fn approval_kind_for_tool(call: &AgentToolCall) -> ApprovalKind {
    match call.name.as_str() {
        "exec" | "process" => ApprovalKind::CommandExecution,
        "apply_patch" | "write" | "edit" => ApprovalKind::FileChange,
        _ => ApprovalKind::Other,
    }
}

fn debug_enabled() -> bool {
    matches!(std::env::var("HOMIE_DEBUG").as_deref(), Ok("1"))
        || matches!(std::env::var("HOME_DEBUG").as_deref(), Ok("1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures::stream::{self, BoxStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::{timeout, Duration};

    use crate::agent::message::{convert_to_llm, AgentMessage};
    use crate::agent_loop::RunStatus;
    use crate::models::ModelCapabilities;
    use crate::provider::{ModelProvider, ProviderResponse};
    use crate::tools::arguments::ToolArguments;
    use crate::tools::tool::{AgentTool, ToolExecutionContext, ToolUpdateCallback};
    use crate::tools::types::AgentToolParameters;
    use crate::types::{ContentPart, Usage};

    #[derive(Clone, Copy)]
    enum ProviderScenario {
        MissingOptionalFields,
        TextThenStreamError,
        RepeatedToolFailure,
        RateLimitedThenComplete,
        RateLimitedExceedsCap,
        RateLimitedWithoutRetryHint,
        ParallelSafeBatchThenComplete,
        MutatingBatchThenComplete,
        MixedTextAndParallelBatchThenComplete,
        DuplicateToolCallDeltaThenComplete,
        StreamEndsWithoutDoneThenComplete,
        ToolUpdateThenComplete,
        /// Tool call for "schema_tool" with empty args on call 0, then text "done" on call 1+.
        SchemaToolBadArgs,
        /// Tool call for "schema_tool" with valid args on call 0, then text "done" on call 1+.
        SchemaToolValidArgs,
        /// Tool call for "schema_tool" with type-mismatched args on call 0, then text "done" on call 1+.
        SchemaToolTypeMismatch,
    }

    struct StubProvider {
        scenario: ProviderScenario,
        calls: AtomicUsize,
        capabilities: ModelCapabilities,
        requests: Arc<std::sync::Mutex<Vec<ProviderRequest>>>,
    }

    impl StubProvider {
        fn new(
            scenario: ProviderScenario,
            requests: Arc<std::sync::Mutex<Vec<ProviderRequest>>>,
        ) -> Self {
            Self {
                scenario,
                calls: AtomicUsize::new(0),
                capabilities: ModelCapabilities::default(),
                requests,
            }
        }
    }

    #[async_trait]
    impl ModelProvider for StubProvider {
        fn provider_name(&self) -> &str {
            "stub"
        }

        fn model_id(&self) -> &str {
            "stub-model"
        }

        fn capabilities(&self) -> &ModelCapabilities {
            &self.capabilities
        }

        async fn generate_text(
            &self,
            _request: &ProviderRequest,
        ) -> Result<ProviderResponse, RociError> {
            Err(RociError::UnsupportedOperation(
                "stream-only stub provider".to_string(),
            ))
        }

        async fn stream_text(
            &self,
            request: &ProviderRequest,
        ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
            self.requests
                .lock()
                .expect("request lock")
                .push(request.clone());
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
            let events = match self.scenario {
                ProviderScenario::MissingOptionalFields => vec![
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Reasoning,
                        tool_call: None,
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: None,
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: "done".to_string(),
                        event_type: StreamEventType::TextDelta,
                        tool_call: None,
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Done,
                        tool_call: None,
                        finish_reason: None,
                        usage: Some(Usage::default()),
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                ],
                ProviderScenario::TextThenStreamError => vec![
                    Ok(TextStreamDelta {
                        text: "partial".to_string(),
                        event_type: StreamEventType::TextDelta,
                        tool_call: None,
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: "upstream stream failure".to_string(),
                        event_type: StreamEventType::Error,
                        tool_call: None,
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                ],
                ProviderScenario::RepeatedToolFailure => vec![
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: Some(AgentToolCall {
                            id: "tool-call-1".to_string(),
                            name: "failing_tool".to_string(),
                            arguments: serde_json::json!({}),
                            recipient: None,
                        }),
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Done,
                        tool_call: None,
                        finish_reason: None,
                        usage: Some(Usage::default()),
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                ],
                ProviderScenario::RateLimitedThenComplete => {
                    if call_index == 0 {
                        return Err(RociError::RateLimited {
                            retry_after_ms: Some(1),
                        });
                    }
                    vec![Ok(TextStreamDelta {
                        text: "done".to_string(),
                        event_type: StreamEventType::TextDelta,
                        tool_call: None,
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    })]
                }
                ProviderScenario::RateLimitedExceedsCap => {
                    return Err(RociError::RateLimited {
                        retry_after_ms: Some(50),
                    });
                }
                ProviderScenario::RateLimitedWithoutRetryHint => {
                    return Err(RociError::RateLimited {
                        retry_after_ms: None,
                    });
                }
                ProviderScenario::ParallelSafeBatchThenComplete => {
                    if call_index == 0 {
                        vec![
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::ToolCallDelta,
                                tool_call: Some(AgentToolCall {
                                    id: "safe-read-1".to_string(),
                                    name: "read".to_string(),
                                    arguments: serde_json::json!({}),
                                    recipient: None,
                                }),
                                finish_reason: None,
                                usage: None,
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::ToolCallDelta,
                                tool_call: Some(AgentToolCall {
                                    id: "safe-ls-2".to_string(),
                                    name: "ls".to_string(),
                                    arguments: serde_json::json!({}),
                                    recipient: None,
                                }),
                                finish_reason: None,
                                usage: None,
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::Done,
                                tool_call: None,
                                finish_reason: None,
                                usage: Some(Usage::default()),
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                        ]
                    } else {
                        vec![Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::Done,
                            tool_call: None,
                            finish_reason: None,
                            usage: Some(Usage::default()),
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        })]
                    }
                }
                ProviderScenario::MutatingBatchThenComplete => {
                    if call_index == 0 {
                        vec![
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::ToolCallDelta,
                                tool_call: Some(AgentToolCall {
                                    id: "mutating-call-1".to_string(),
                                    name: "apply_patch".to_string(),
                                    arguments: serde_json::json!({}),
                                    recipient: None,
                                }),
                                finish_reason: None,
                                usage: None,
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::ToolCallDelta,
                                tool_call: Some(AgentToolCall {
                                    id: "safe-read-2".to_string(),
                                    name: "read".to_string(),
                                    arguments: serde_json::json!({}),
                                    recipient: None,
                                }),
                                finish_reason: None,
                                usage: None,
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::Done,
                                tool_call: None,
                                finish_reason: None,
                                usage: Some(Usage::default()),
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                        ]
                    } else {
                        vec![Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::Done,
                            tool_call: None,
                            finish_reason: None,
                            usage: Some(Usage::default()),
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        })]
                    }
                }
                ProviderScenario::MixedTextAndParallelBatchThenComplete => {
                    if call_index == 0 {
                        vec![
                            Ok(TextStreamDelta {
                                text: "Gathering context.".to_string(),
                                event_type: StreamEventType::TextDelta,
                                tool_call: None,
                                finish_reason: None,
                                usage: None,
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::ToolCallDelta,
                                tool_call: Some(AgentToolCall {
                                    id: "mixed-read-1".to_string(),
                                    name: "read".to_string(),
                                    arguments: serde_json::json!({ "path": "README.md" }),
                                    recipient: None,
                                }),
                                finish_reason: None,
                                usage: None,
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::ToolCallDelta,
                                tool_call: Some(AgentToolCall {
                                    id: "mixed-ls-2".to_string(),
                                    name: "ls".to_string(),
                                    arguments: serde_json::json!({ "path": "." }),
                                    recipient: None,
                                }),
                                finish_reason: None,
                                usage: None,
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::Done,
                                tool_call: None,
                                finish_reason: None,
                                usage: Some(Usage::default()),
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                        ]
                    } else {
                        vec![
                            Ok(TextStreamDelta {
                                text: "complete".to_string(),
                                event_type: StreamEventType::TextDelta,
                                tool_call: None,
                                finish_reason: None,
                                usage: None,
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::Done,
                                tool_call: None,
                                finish_reason: None,
                                usage: Some(Usage::default()),
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                        ]
                    }
                }
                ProviderScenario::DuplicateToolCallDeltaThenComplete => {
                    if call_index == 0 {
                        vec![
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::ToolCallDelta,
                                tool_call: Some(AgentToolCall {
                                    id: "dup-read-1".to_string(),
                                    name: "read".to_string(),
                                    arguments: serde_json::json!({ "path": "first" }),
                                    recipient: None,
                                }),
                                finish_reason: None,
                                usage: None,
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::ToolCallDelta,
                                tool_call: Some(AgentToolCall {
                                    id: "dup-read-1".to_string(),
                                    name: "read".to_string(),
                                    arguments: serde_json::json!({ "path": "second" }),
                                    recipient: None,
                                }),
                                finish_reason: None,
                                usage: None,
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::Done,
                                tool_call: None,
                                finish_reason: None,
                                usage: Some(Usage::default()),
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                        ]
                    } else {
                        vec![Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::Done,
                            tool_call: None,
                            finish_reason: None,
                            usage: Some(Usage::default()),
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        })]
                    }
                }
                ProviderScenario::StreamEndsWithoutDoneThenComplete => {
                    if call_index == 0 {
                        vec![Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::ToolCallDelta,
                            tool_call: Some(AgentToolCall {
                                id: "fallback-read-1".to_string(),
                                name: "read".to_string(),
                                arguments: serde_json::json!({ "path": "fallback" }),
                                recipient: None,
                            }),
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        })]
                    } else {
                        vec![Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::Done,
                            tool_call: None,
                            finish_reason: None,
                            usage: Some(Usage::default()),
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        })]
                    }
                }
                ProviderScenario::ToolUpdateThenComplete => {
                    if call_index == 0 {
                        vec![
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::ToolCallDelta,
                                tool_call: Some(AgentToolCall {
                                    id: "update-tool-1".to_string(),
                                    name: "update_tool".to_string(),
                                    arguments: serde_json::json!({ "path": "README.md" }),
                                    recipient: None,
                                }),
                                finish_reason: None,
                                usage: None,
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::Done,
                                tool_call: None,
                                finish_reason: None,
                                usage: Some(Usage::default()),
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                        ]
                    } else {
                        vec![
                            Ok(TextStreamDelta {
                                text: "done".to_string(),
                                event_type: StreamEventType::TextDelta,
                                tool_call: None,
                                finish_reason: None,
                                usage: None,
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::Done,
                                tool_call: None,
                                finish_reason: None,
                                usage: Some(Usage::default()),
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                        ]
                    }
                }
                ProviderScenario::SchemaToolBadArgs
                | ProviderScenario::SchemaToolValidArgs
                | ProviderScenario::SchemaToolTypeMismatch => {
                    let args = match self.scenario {
                        ProviderScenario::SchemaToolBadArgs => serde_json::json!({}),
                        ProviderScenario::SchemaToolValidArgs => {
                            serde_json::json!({ "path": "/tmp/test" })
                        }
                        ProviderScenario::SchemaToolTypeMismatch => {
                            serde_json::json!({ "path": 42 })
                        }
                        _ => unreachable!(),
                    };
                    if call_index == 0 {
                        vec![
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::ToolCallDelta,
                                tool_call: Some(AgentToolCall {
                                    id: "schema-call-1".to_string(),
                                    name: "schema_tool".to_string(),
                                    arguments: args,
                                    recipient: None,
                                }),
                                finish_reason: None,
                                usage: None,
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::Done,
                                tool_call: None,
                                finish_reason: None,
                                usage: Some(Usage::default()),
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                        ]
                    } else {
                        vec![
                            Ok(TextStreamDelta {
                                text: "done".to_string(),
                                event_type: StreamEventType::TextDelta,
                                tool_call: None,
                                finish_reason: None,
                                usage: None,
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                            Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::Done,
                                tool_call: None,
                                finish_reason: None,
                                usage: Some(Usage::default()),
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            }),
                        ]
                    }
                }
            };
            Ok(Box::pin(stream::iter(events)))
        }
    }

    fn test_runner(
        scenario: ProviderScenario,
    ) -> (LoopRunner, Arc<std::sync::Mutex<Vec<ProviderRequest>>>) {
        let requests = Arc::new(std::sync::Mutex::new(Vec::<ProviderRequest>::new()));
        let provider_requests = requests.clone();
        let factory: ProviderFactory = Arc::new(move |_model, _config| {
            Ok(Box::new(StubProvider::new(
                scenario,
                provider_requests.clone(),
            )))
        });
        (
            LoopRunner::with_provider_factory(RociConfig::new(), factory),
            requests,
        )
    }

    fn test_model() -> LanguageModel {
        LanguageModel::Custom {
            provider: "stub".to_string(),
            model_id: "stub-model".to_string(),
        }
    }

    fn capture_events() -> (RunEventSink, Arc<std::sync::Mutex<Vec<RunEvent>>>) {
        let events = Arc::new(std::sync::Mutex::new(Vec::<RunEvent>::new()));
        let sink_events = events.clone();
        let sink: RunEventSink = Arc::new(move |event| {
            if let Ok(mut guard) = sink_events.lock() {
                guard.push(event);
            }
        });
        (sink, events)
    }

    fn capture_agent_events() -> (AgentEventSink, Arc<std::sync::Mutex<Vec<AgentEvent>>>) {
        let events = Arc::new(std::sync::Mutex::new(Vec::<AgentEvent>::new()));
        let sink_events = events.clone();
        let sink: AgentEventSink = Arc::new(move |event| {
            if let Ok(mut guard) = sink_events.lock() {
                guard.push(event);
            }
        });
        (sink, events)
    }

    struct UpdateStreamingTool {
        params: AgentToolParameters,
        wait_for_cancel: bool,
    }

    impl UpdateStreamingTool {
        fn new(wait_for_cancel: bool) -> Self {
            Self {
                params: AgentToolParameters::empty(),
                wait_for_cancel,
            }
        }
    }

    #[async_trait]
    impl Tool for UpdateStreamingTool {
        fn name(&self) -> &str {
            "update_tool"
        }

        fn description(&self) -> &str {
            "tool that emits partial updates"
        }

        fn parameters(&self) -> &AgentToolParameters {
            &self.params
        }

        async fn execute(
            &self,
            _args: &ToolArguments,
            _ctx: &ToolExecutionContext,
        ) -> Result<serde_json::Value, RociError> {
            Ok(serde_json::json!({ "tool": "update_tool", "status": "ok" }))
        }

        async fn execute_ext(
            &self,
            _args: &ToolArguments,
            _ctx: &ToolExecutionContext,
            cancel: CancellationToken,
            on_update: Option<ToolUpdateCallback>,
        ) -> Result<serde_json::Value, RociError> {
            if let Some(callback) = on_update.as_ref() {
                callback(ToolUpdatePayload {
                    content: vec![ContentPart::Text {
                        text: "partial-1".to_string(),
                    }],
                    details: serde_json::json!({ "step": 1 }),
                });
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
            if let Some(callback) = on_update.as_ref() {
                callback(ToolUpdatePayload {
                    content: vec![ContentPart::Text {
                        text: "partial-2".to_string(),
                    }],
                    details: serde_json::json!({ "step": 2 }),
                });
            }

            if self.wait_for_cancel {
                tokio::select! {
                    _ = cancel.cancelled() => Err(RociError::ToolExecution {
                        tool_name: "update_tool".to_string(),
                        message: "canceled".to_string(),
                    }),
                    _ = tokio::time::sleep(Duration::from_secs(5)) => Ok(serde_json::json!({
                        "tool": "update_tool",
                        "status": "late_ok",
                    })),
                }
            } else {
                Ok(serde_json::json!({
                    "tool": "update_tool",
                    "status": "ok",
                }))
            }
        }
    }

    fn update_streaming_tool(wait_for_cancel: bool) -> Arc<dyn Tool> {
        Arc::new(UpdateStreamingTool::new(wait_for_cancel))
    }

    fn failing_tool() -> Arc<dyn Tool> {
        Arc::new(AgentTool::new(
            "failing_tool",
            "always fails",
            AgentToolParameters::empty(),
            |_args, _ctx: ToolExecutionContext| async move {
                Err(RociError::ToolExecution {
                    tool_name: "failing_tool".to_string(),
                    message: "forced failure".to_string(),
                })
            },
        ))
    }

    fn tracked_success_tool(
        name: &str,
        delay: Duration,
        active_calls: Arc<AtomicUsize>,
        max_active_calls: Arc<AtomicUsize>,
    ) -> Arc<dyn Tool> {
        let tool_name = name.to_string();
        Arc::new(AgentTool::new(
            tool_name.clone(),
            format!("{tool_name} tool"),
            AgentToolParameters::empty(),
            move |_args, _ctx: ToolExecutionContext| {
                let tool_name = tool_name.clone();
                let active_calls = active_calls.clone();
                let max_active_calls = max_active_calls.clone();
                async move {
                    let active_now = active_calls.fetch_add(1, Ordering::SeqCst) + 1;
                    let mut observed_max = max_active_calls.load(Ordering::SeqCst);
                    while active_now > observed_max {
                        match max_active_calls.compare_exchange(
                            observed_max,
                            active_now,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        ) {
                            Ok(_) => break,
                            Err(next) => observed_max = next,
                        }
                    }
                    tokio::time::sleep(delay).await;
                    active_calls.fetch_sub(1, Ordering::SeqCst);
                    Ok(serde_json::json!({ "tool": tool_name }))
                }
            },
        ))
    }

    fn tool_result_ids_from_messages(messages: &[ModelMessage]) -> Vec<String> {
        messages
            .iter()
            .filter_map(|message| {
                message.content.iter().find_map(|part| match part {
                    ContentPart::ToolResult(result) => Some(result.tool_call_id.clone()),
                    _ => None,
                })
            })
            .collect()
    }

    fn assistant_tool_call_message_count(messages: &[ModelMessage]) -> usize {
        messages
            .iter()
            .filter(|message| {
                matches!(message.role, crate::types::Role::Assistant)
                    && message
                        .content
                        .iter()
                        .any(|part| matches!(part, ContentPart::ToolCall(_)))
            })
            .count()
    }

    fn assistant_tool_calls(messages: &[ModelMessage]) -> Vec<AgentToolCall> {
        messages
            .iter()
            .filter(|message| matches!(message.role, crate::types::Role::Assistant))
            .flat_map(|message| {
                message.content.iter().filter_map(|part| match part {
                    ContentPart::ToolCall(call) => Some(call.clone()),
                    _ => None,
                })
            })
            .collect()
    }

    fn assistant_text_content(messages: &[ModelMessage]) -> String {
        messages
            .iter()
            .filter(|message| matches!(message.role, crate::types::Role::Assistant))
            .flat_map(|message| {
                message.content.iter().filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn tool_result_ids_from_events(events: &[RunEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match &event.payload {
                RunEventPayload::ToolResult { result } => Some(result.tool_call_id.clone()),
                _ => None,
            })
            .collect()
    }

    fn tool_call_completed_ids_from_events(events: &[RunEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match &event.payload {
                RunEventPayload::ToolCallCompleted { call } => Some(call.id.clone()),
                _ => None,
            })
            .collect()
    }

    fn tool_result_id_from_message(message: &ModelMessage) -> Option<&str> {
        message.content.iter().find_map(|part| match part {
            ContentPart::ToolResult(result) => Some(result.tool_call_id.as_str()),
            _ => None,
        })
    }

    #[tokio::test]
    async fn no_panic_when_stream_optional_fields_missing() {
        let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
        let (sink, _events) = capture_events();
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
        request.event_sink = Some(sink);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Completed);
        assert!(
            !result.messages.is_empty(),
            "completed runs should carry final conversation messages"
        );
        assert!(
            result
                .messages
                .iter()
                .any(|message| matches!(message.role, crate::types::Role::User)),
            "result should include persisted prompt context"
        );
    }

    #[tokio::test]
    async fn agent_message_lifecycle_events_emit_for_text_stream() {
        let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
        let (agent_sink, agent_events) = capture_agent_events();
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
        request.agent_event_sink = Some(agent_sink);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Completed);

        let events = agent_events.lock().expect("agent event lock");
        let start_idx = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::MessageStart { message }
                        if message.role == crate::types::Role::Assistant
                )
            })
            .expect("expected assistant MessageStart");
        let update_idx = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::MessageUpdate {
                        message,
                        assistant_message_event,
                        ..
                    } if message.role == crate::types::Role::Assistant
                        && assistant_message_event.event_type == StreamEventType::TextDelta
                        && assistant_message_event.text == "done"
                )
            })
            .expect("expected MessageUpdate(done)");
        let end_idx = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::MessageEnd { message }
                        if message.role == crate::types::Role::Assistant
                )
            })
            .expect("expected assistant MessageEnd");
        assert!(start_idx < update_idx);
        assert!(update_idx < end_idx);
    }

    #[tokio::test]
    async fn message_lifecycle_events_cover_prompt_and_tool_results() {
        let (runner, _requests) = test_runner(ProviderScenario::ToolUpdateThenComplete);
        let (agent_sink, agent_events) = capture_agent_events();
        let mut request =
            RunRequest::new(test_model(), vec![ModelMessage::user("run update tool")]);
        request.tools = vec![update_streaming_tool(false)];
        request.approval_policy = ApprovalPolicy::Always;
        request.agent_event_sink = Some(agent_sink);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(3), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Completed);

        let events = agent_events.lock().expect("agent event lock");
        let user_start_count = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    AgentEvent::MessageStart { message }
                        if message.role == crate::types::Role::User
                )
            })
            .count();
        let user_end_count = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    AgentEvent::MessageEnd { message }
                        if message.role == crate::types::Role::User
                )
            })
            .count();
        assert_eq!(user_start_count, 1);
        assert_eq!(user_end_count, 1);

        let tool_start = events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::MessageStart { message }
                    if message.role == crate::types::Role::Tool
                        && tool_result_id_from_message(message) == Some("update-tool-1")
            )
        });
        let tool_end = events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::MessageEnd { message }
                    if message.role == crate::types::Role::Tool
                        && tool_result_id_from_message(message) == Some("update-tool-1")
            )
        });
        assert!(
            tool_start,
            "expected tool result MessageStart for update-tool-1"
        );
        assert!(
            tool_end,
            "expected tool result MessageEnd for update-tool-1"
        );
    }

    #[tokio::test]
    async fn agent_message_end_is_emitted_before_failure_terminal_event() {
        let (runner, _requests) = test_runner(ProviderScenario::TextThenStreamError);
        let (agent_sink, agent_events) = capture_agent_events();
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
        request.agent_event_sink = Some(agent_sink);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Failed);
        assert!(result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("upstream stream failure"));

        let events = agent_events.lock().expect("agent event lock");
        let start_idx = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::MessageStart { message }
                        if message.role == crate::types::Role::Assistant
                )
            })
            .expect("expected assistant MessageStart");
        let update_idx = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::MessageUpdate {
                        message,
                        assistant_message_event,
                        ..
                    } if message.role == crate::types::Role::Assistant
                        && assistant_message_event.event_type == StreamEventType::TextDelta
                        && assistant_message_event.text == "partial"
                )
            })
            .expect("expected MessageUpdate(partial)");
        let message_end_idx = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::MessageEnd { message }
                        if message.role == crate::types::Role::Assistant
                )
            })
            .expect("expected assistant MessageEnd");
        let agent_end_idx = events
            .iter()
            .position(|event| matches!(event, AgentEvent::AgentEnd { .. }))
            .expect("expected AgentEnd");
        assert!(start_idx < update_idx);
        assert!(update_idx < message_end_idx);
        assert!(message_end_idx < agent_end_idx);
    }

    #[tokio::test]
    async fn tool_execution_updates_stream_with_deterministic_order() {
        let (runner, _requests) = test_runner(ProviderScenario::ToolUpdateThenComplete);
        let (agent_sink, agent_events) = capture_agent_events();
        let mut request =
            RunRequest::new(test_model(), vec![ModelMessage::user("run update tool")]);
        request.tools = vec![update_streaming_tool(false)];
        request.approval_policy = ApprovalPolicy::Always;
        request.agent_event_sink = Some(agent_sink);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(3), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Completed);
        assert_eq!(
            tool_result_ids_from_messages(&result.messages),
            vec!["update-tool-1".to_string()]
        );

        let events = agent_events.lock().expect("agent event lock");
        let mut sequence: Vec<String> = Vec::new();
        for event in events.iter() {
            match event {
                AgentEvent::ToolExecutionStart {
                    tool_call_id,
                    tool_name,
                    ..
                } if tool_call_id == "update-tool-1" && tool_name == "update_tool" => {
                    sequence.push("start".to_string());
                }
                AgentEvent::ToolExecutionUpdate {
                    tool_call_id,
                    partial_result,
                    ..
                } if tool_call_id == "update-tool-1" => {
                    let step = partial_result
                        .details
                        .get("step")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or_default();
                    sequence.push(format!("update-{step}"));
                }
                AgentEvent::ToolExecutionEnd {
                    tool_call_id,
                    is_error,
                    ..
                } if tool_call_id == "update-tool-1" => {
                    assert!(!is_error);
                    sequence.push("end".to_string());
                }
                _ => {}
            }
        }
        assert_eq!(
            sequence,
            vec![
                "start".to_string(),
                "update-1".to_string(),
                "update-2".to_string(),
                "end".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn canceling_during_tool_execution_emits_error_end_event() {
        let (runner, _requests) = test_runner(ProviderScenario::ToolUpdateThenComplete);
        let (agent_sink, agent_events) = capture_agent_events();
        let mut request =
            RunRequest::new(test_model(), vec![ModelMessage::user("cancel update tool")]);
        request.tools = vec![update_streaming_tool(true)];
        request.approval_policy = ApprovalPolicy::Always;
        request.agent_event_sink = Some(agent_sink);

        let mut handle = runner.start(request).await.expect("start run");
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(handle.abort(), "abort should be accepted");

        let result = timeout(Duration::from_secs(3), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Canceled);

        let events = agent_events.lock().expect("agent event lock");
        let end_event = events.iter().find_map(|event| match event {
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                is_error,
                ..
            } if tool_call_id == "update-tool-1" => Some(*is_error),
            _ => None,
        });
        assert_eq!(end_event, Some(true));
    }

    #[tokio::test]
    async fn steering_skip_emits_tool_and_message_lifecycle_for_skipped_calls() {
        let (runner, _requests) = test_runner(ProviderScenario::MutatingBatchThenComplete);
        let (agent_sink, agent_events) = capture_agent_events();
        let active_calls = Arc::new(AtomicUsize::new(0));
        let max_active_calls = Arc::new(AtomicUsize::new(0));
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("run tools")]);
        request.tools = vec![
            tracked_success_tool(
                "apply_patch",
                Duration::from_millis(40),
                active_calls.clone(),
                max_active_calls.clone(),
            ),
            tracked_success_tool(
                "read",
                Duration::from_millis(40),
                active_calls,
                max_active_calls,
            ),
        ];
        request.approval_policy = ApprovalPolicy::Always;
        request.agent_event_sink = Some(agent_sink);

        let steering_tick = Arc::new(AtomicUsize::new(0));
        let steering_tick_clone = steering_tick.clone();
        request.get_steering_messages = Some(Arc::new(move || {
            let tick = steering_tick_clone.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move {
                if tick >= 2 {
                    vec![ModelMessage::user("interrupt")]
                } else {
                    Vec::new()
                }
            })
        }));

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(4), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Completed);
        assert!(
            tool_result_ids_from_messages(&result.messages)
                .iter()
                .any(|id| id == "safe-read-2"),
            "expected skipped tool result for safe-read-2"
        );

        let events = agent_events.lock().expect("agent event lock");
        let skipped_tool_start = events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::ToolExecutionStart { tool_call_id, .. } if tool_call_id == "safe-read-2"
            )
        });
        let skipped_tool_end = events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::ToolExecutionEnd { tool_call_id, .. } if tool_call_id == "safe-read-2"
            )
        });
        let skipped_msg_start = events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::MessageStart { message }
                    if message.role == crate::types::Role::Tool
                        && tool_result_id_from_message(message) == Some("safe-read-2")
            )
        });
        let skipped_msg_end = events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::MessageEnd { message }
                    if message.role == crate::types::Role::Tool
                        && tool_result_id_from_message(message) == Some("safe-read-2")
            )
        });

        assert!(
            skipped_tool_start,
            "expected ToolExecutionStart for skipped call"
        );
        assert!(
            skipped_tool_end,
            "expected ToolExecutionEnd for skipped call"
        );
        assert!(
            skipped_msg_start,
            "expected MessageStart for skipped tool result"
        );
        assert!(
            skipped_msg_end,
            "expected MessageEnd for skipped tool result"
        );
    }

    #[tokio::test]
    async fn tool_failures_are_bounded_with_deterministic_reason() {
        let (runner, _requests) = test_runner(ProviderScenario::RepeatedToolFailure);
        let (sink, events) = capture_events();
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("run tool")]);
        request.tools = vec![failing_tool()];
        request.approval_policy = ApprovalPolicy::Always;
        request.event_sink = Some(sink);
        request
            .metadata
            .insert("runner.max_iterations".to_string(), "20".to_string());
        request
            .metadata
            .insert("runner.max_tool_failures".to_string(), "2".to_string());

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("run wait timeout");

        let expected_error =
            "tool call failure limit reached (max_failures=2, consecutive_failures=2)";
        assert_eq!(result.status, RunStatus::Failed);
        assert_eq!(result.error.as_deref(), Some(expected_error));
        assert!(
            !result.messages.is_empty(),
            "failed runs should still expose conversation state"
        );
        let result_tool_ids = tool_result_ids_from_messages(&result.messages);
        assert_eq!(result_tool_ids.len(), 2);
        assert!(result_tool_ids.iter().all(|id| id == "tool-call-1"));

        let events = events.lock().expect("event lock");
        let failure_events: Vec<String> = events
            .iter()
            .filter_map(|event| match &event.payload {
                RunEventPayload::Lifecycle {
                    state: RunLifecycle::Failed { error },
                } => Some(error.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            failure_events.last().map(String::as_str),
            Some(expected_error)
        );

        let tool_results = events
            .iter()
            .filter(|event| matches!(event.payload, RunEventPayload::ToolResult { .. }))
            .count();
        assert_eq!(tool_results, 2);
    }

    #[tokio::test]
    async fn request_transport_is_forwarded_to_provider_request() {
        let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
        request.transport = Some(provider::TRANSPORT_PROXY.to_string());

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Completed);

        let requests = requests.lock().expect("request lock");
        assert!(
            !requests.is_empty(),
            "provider should receive at least one request"
        );
        assert_eq!(
            requests[0].transport.as_deref(),
            Some(provider::TRANSPORT_PROXY)
        );
    }

    #[tokio::test]
    async fn unsupported_request_transport_is_rejected_before_provider_call() {
        let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
        request.transport = Some("satellite".to_string());

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Failed);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("unsupported provider transport 'satellite'"),
            "expected unsupported transport error, got: {:?}",
            result.error
        );

        let requests = requests.lock().expect("request lock");
        assert!(
            requests.is_empty(),
            "provider should not be called for unsupported transports"
        );
    }

    #[tokio::test]
    async fn convert_to_llm_hook_can_append_and_filter_custom_messages() {
        let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
        request.convert_to_llm = Some(Arc::new(|mut messages: Vec<AgentMessage>| {
            Box::pin(async move {
                messages.push(AgentMessage::custom(
                    "artifact",
                    serde_json::json!({ "hidden": true }),
                ));
                messages.push(AgentMessage::user("hook-added"));
                convert_to_llm(&messages)
            })
        }));

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Completed);

        let requests = requests.lock().expect("request lock");
        assert!(!requests.is_empty(), "provider should receive one request");
        let first = &requests[0].messages;
        assert!(
            first.iter().any(|m| m.text() == "hook-added"),
            "conversion hook should be able to append LLM-visible messages"
        );
        assert!(
            first.iter().all(|m| matches!(
                m.role,
                crate::types::Role::System
                    | crate::types::Role::User
                    | crate::types::Role::Assistant
                    | crate::types::Role::Tool
            )),
            "provider messages must remain LLM message roles after conversion"
        );
    }

    #[tokio::test]
    async fn rate_limited_stream_retries_within_max_delay_cap() {
        let (runner, requests) = test_runner(ProviderScenario::RateLimitedThenComplete);
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("retry")]);
        request.max_retry_delay_ms = Some(10);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Completed);

        let requests = requests.lock().expect("request lock");
        assert_eq!(requests.len(), 2);
    }

    #[tokio::test]
    async fn rate_limited_stream_fails_when_retry_delay_exceeds_cap() {
        let (runner, requests) = test_runner(ProviderScenario::RateLimitedExceedsCap);
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("retry")]);
        request.max_retry_delay_ms = Some(10);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Failed);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("exceeds max_retry_delay_ms"),
            "expected max retry delay failure, got: {:?}",
            result.error
        );

        let requests = requests.lock().expect("request lock");
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn rate_limited_without_retry_hint_fails_immediately() {
        let (runner, requests) = test_runner(ProviderScenario::RateLimitedWithoutRetryHint);
        let request = RunRequest::new(test_model(), vec![ModelMessage::user("retry")]);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Failed);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("without retry_after hint"),
            "expected missing retry hint failure, got: {:?}",
            result.error
        );

        let requests = requests.lock().expect("request lock");
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn parallel_safe_tools_execute_concurrently_and_append_results_in_call_order() {
        let (runner, requests) = test_runner(ProviderScenario::ParallelSafeBatchThenComplete);
        let (sink, events) = capture_events();
        let active_calls = Arc::new(AtomicUsize::new(0));
        let max_active_calls = Arc::new(AtomicUsize::new(0));
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("parallel tools")]);
        request.tools = vec![
            tracked_success_tool(
                "read",
                Duration::from_millis(150),
                active_calls.clone(),
                max_active_calls.clone(),
            ),
            tracked_success_tool(
                "ls",
                Duration::from_millis(150),
                active_calls,
                max_active_calls.clone(),
            ),
        ];
        request.approval_policy = ApprovalPolicy::Always;
        request.event_sink = Some(sink);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(3), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Completed);
        assert!(max_active_calls.load(Ordering::SeqCst) >= 2);

        let requests = requests.lock().expect("request lock");
        assert!(requests.len() >= 2);
        let second_request_messages = &requests[1].messages;
        assert_eq!(
            assistant_tool_call_message_count(second_request_messages),
            1
        );
        assert_eq!(
            tool_result_ids_from_messages(second_request_messages),
            vec!["safe-read-1".to_string(), "safe-ls-2".to_string()]
        );

        let events = events.lock().expect("event lock");
        assert_eq!(
            tool_result_ids_from_events(events.as_slice()),
            vec!["safe-read-1".to_string(), "safe-ls-2".to_string()]
        );
        assert_eq!(
            tool_call_completed_ids_from_events(events.as_slice()),
            vec!["safe-read-1".to_string(), "safe-ls-2".to_string()]
        );
    }

    #[tokio::test]
    async fn mutating_tools_remain_serialized_even_when_safe_tools_exist() {
        let (runner, requests) = test_runner(ProviderScenario::MutatingBatchThenComplete);
        let (sink, events) = capture_events();
        let active_calls = Arc::new(AtomicUsize::new(0));
        let max_active_calls = Arc::new(AtomicUsize::new(0));
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("mutating tools")]);
        request.tools = vec![
            tracked_success_tool(
                "apply_patch",
                Duration::from_millis(150),
                active_calls.clone(),
                max_active_calls.clone(),
            ),
            tracked_success_tool(
                "read",
                Duration::from_millis(150),
                active_calls,
                max_active_calls.clone(),
            ),
        ];
        request.approval_policy = ApprovalPolicy::Always;
        request.event_sink = Some(sink);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(3), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Completed);
        assert_eq!(max_active_calls.load(Ordering::SeqCst), 1);

        let requests = requests.lock().expect("request lock");
        assert!(requests.len() >= 2);
        let second_request_messages = &requests[1].messages;
        assert_eq!(
            assistant_tool_call_message_count(second_request_messages),
            1
        );
        assert_eq!(
            tool_result_ids_from_messages(second_request_messages),
            vec!["mutating-call-1".to_string(), "safe-read-2".to_string()]
        );

        let events = events.lock().expect("event lock");
        assert_eq!(
            tool_result_ids_from_events(events.as_slice()),
            vec!["mutating-call-1".to_string(), "safe-read-2".to_string()]
        );
        assert_eq!(
            tool_call_completed_ids_from_events(events.as_slice()),
            vec!["mutating-call-1".to_string(), "safe-read-2".to_string()]
        );
    }

    #[tokio::test]
    async fn mixed_text_and_parallel_tools_are_batched_before_single_followup() {
        let (runner, requests) =
            test_runner(ProviderScenario::MixedTextAndParallelBatchThenComplete);
        let (sink, events) = capture_events();
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("mixed stream")]);
        request.tools = vec![
            tracked_success_tool(
                "read",
                Duration::from_millis(80),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
            ),
            tracked_success_tool(
                "ls",
                Duration::from_millis(80),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
            ),
        ];
        request.approval_policy = ApprovalPolicy::Always;
        request.event_sink = Some(sink);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(3), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Completed);

        let requests = requests.lock().expect("request lock");
        assert_eq!(requests.len(), 2);
        let second_request_messages = &requests[1].messages;
        assert_eq!(
            assistant_tool_call_message_count(second_request_messages),
            1
        );
        assert_eq!(
            tool_result_ids_from_messages(second_request_messages),
            vec!["mixed-read-1".to_string(), "mixed-ls-2".to_string()]
        );
        assert!(assistant_text_content(second_request_messages).contains("Gathering context."));

        let events = events.lock().expect("event lock");
        assert_eq!(
            tool_result_ids_from_events(events.as_slice()),
            vec!["mixed-read-1".to_string(), "mixed-ls-2".to_string()]
        );
    }

    #[tokio::test]
    async fn duplicate_tool_call_deltas_are_deduplicated_by_call_id() {
        let (runner, requests) = test_runner(ProviderScenario::DuplicateToolCallDeltaThenComplete);
        let (sink, events) = capture_events();
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("dup tool call")]);
        request.tools = vec![tracked_success_tool(
            "read",
            Duration::from_millis(50),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        )];
        request.approval_policy = ApprovalPolicy::Always;
        request.event_sink = Some(sink);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(3), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Completed);

        let requests = requests.lock().expect("request lock");
        assert_eq!(requests.len(), 2);
        let second_request_messages = &requests[1].messages;
        assert_eq!(
            tool_result_ids_from_messages(second_request_messages),
            vec!["dup-read-1".to_string()]
        );
        let calls = assistant_tool_calls(second_request_messages);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "dup-read-1");
        assert_eq!(calls[0].arguments["path"], serde_json::json!("second"));

        let events = events.lock().expect("event lock");
        let tool_starts = events
            .iter()
            .filter(|event| matches!(event.payload, RunEventPayload::ToolCallStarted { .. }))
            .count();
        assert_eq!(tool_starts, 1);
        assert_eq!(
            tool_result_ids_from_events(events.as_slice()),
            vec!["dup-read-1".to_string()]
        );
    }

    #[tokio::test]
    async fn stream_end_without_done_falls_back_to_tool_execution_and_completion() {
        let (runner, requests) = test_runner(ProviderScenario::StreamEndsWithoutDoneThenComplete);
        let (sink, events) = capture_events();
        let mut request = RunRequest::new(
            test_model(),
            vec![ModelMessage::user("fallback completion")],
        );
        request.tools = vec![tracked_success_tool(
            "read",
            Duration::from_millis(50),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        )];
        request.approval_policy = ApprovalPolicy::Always;
        request.event_sink = Some(sink);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(3), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Completed);

        let requests = requests.lock().expect("request lock");
        assert_eq!(requests.len(), 2);
        let second_request_messages = &requests[1].messages;
        assert_eq!(
            tool_result_ids_from_messages(second_request_messages),
            vec!["fallback-read-1".to_string()]
        );

        let events = events.lock().expect("event lock");
        assert_eq!(
            tool_result_ids_from_events(events.as_slice()),
            vec!["fallback-read-1".to_string()]
        );
        assert!(
            events.iter().all(|event| {
                !matches!(
                    event.payload,
                    RunEventPayload::Lifecycle {
                        state: RunLifecycle::Failed { .. },
                    }
                )
            }),
            "stream-end fallback should not emit failed lifecycle"
        );
    }

    /// Tool with a required string `path` parameter for schema-validation integration tests.
    fn schema_tool() -> Arc<dyn Tool> {
        Arc::new(AgentTool::new(
            "schema_tool",
            "tool with required path param",
            AgentToolParameters::object()
                .string("path", "file path", true)
                .build(),
            |_args, _ctx: ToolExecutionContext| async move { Ok(serde_json::json!({ "ok": true })) },
        ))
    }

    /// Extract `(tool_call_id, result_json, is_error)` triples from ToolResult events.
    fn tool_results_from_events(events: &[RunEvent]) -> Vec<(String, serde_json::Value, bool)> {
        events
            .iter()
            .filter_map(|event| match &event.payload {
                RunEventPayload::ToolResult { result } => Some((
                    result.tool_call_id.clone(),
                    result.result.clone(),
                    result.is_error,
                )),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn tool_with_schema_rejects_bad_args_through_runner() {
        let (runner, _requests) = test_runner(ProviderScenario::SchemaToolBadArgs);
        let (sink, events) = capture_events();
        let mut request =
            RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
        request.tools = vec![schema_tool()];
        request.approval_policy = ApprovalPolicy::Always;
        request.event_sink = Some(sink);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(3), handle.wait())
            .await
            .expect("run should complete without timeout");

        // The run must not panic and should complete (provider returns text-only on call 1).
        assert_eq!(result.status, RunStatus::Completed);

        let events = events.lock().expect("event lock");
        let tool_results = tool_results_from_events(&events);
        assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
        let (call_id, result_json, is_error) = &tool_results[0];
        assert_eq!(call_id, "schema-call-1");
        assert!(is_error, "validation failure must set is_error: true");
        let error_msg = result_json["error"].as_str().unwrap_or("");
        assert!(
            error_msg.contains("Argument validation failed"),
            "expected validation error prefix, got: {error_msg}"
        );
        assert!(
            error_msg.contains("missing required field 'path'"),
            "expected missing field detail, got: {error_msg}"
        );
    }

    #[tokio::test]
    async fn tool_with_schema_accepts_valid_args_through_runner() {
        let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
        let (sink, events) = capture_events();
        let mut request =
            RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
        request.tools = vec![schema_tool()];
        request.approval_policy = ApprovalPolicy::Always;
        request.event_sink = Some(sink);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(3), handle.wait())
            .await
            .expect("run should complete without timeout");

        assert_eq!(result.status, RunStatus::Completed);

        let events = events.lock().expect("event lock");
        let tool_results = tool_results_from_events(&events);
        assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
        let (call_id, result_json, is_error) = &tool_results[0];
        assert_eq!(call_id, "schema-call-1");
        assert!(!is_error, "valid args must not set is_error");
        assert_eq!(
            result_json["ok"], true,
            "tool handler should execute and return ok"
        );
    }

    #[tokio::test]
    async fn tool_with_type_mismatch_rejects_through_runner() {
        let (runner, _requests) = test_runner(ProviderScenario::SchemaToolTypeMismatch);
        let (sink, events) = capture_events();
        let mut request =
            RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
        request.tools = vec![schema_tool()];
        request.approval_policy = ApprovalPolicy::Always;
        request.event_sink = Some(sink);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(3), handle.wait())
            .await
            .expect("run should complete without timeout");

        assert_eq!(result.status, RunStatus::Completed);

        let events = events.lock().expect("event lock");
        let tool_results = tool_results_from_events(&events);
        assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
        let (call_id, result_json, is_error) = &tool_results[0];
        assert_eq!(call_id, "schema-call-1");
        assert!(is_error, "type mismatch must set is_error: true");
        let error_msg = result_json["error"].as_str().unwrap_or("");
        assert!(
            error_msg.contains("Argument validation failed"),
            "expected validation error prefix, got: {error_msg}"
        );
        assert!(
            error_msg.contains("expected type 'string'"),
            "expected type mismatch detail, got: {error_msg}"
        );
    }

    #[tokio::test]
    async fn auto_compaction_triggers_when_context_exceeds_reserved_window() {
        let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
        request.hooks = RunHooks {
            compaction: Some(Arc::new(move |_messages| {
                let calls_clone = calls_clone.clone();
                Box::pin(async move {
                    calls_clone.fetch_add(1, Ordering::SeqCst);
                    Ok(None)
                })
            })),
            tool_result_persist: None,
        };
        request.auto_compaction = Some(AutoCompactionConfig {
            reserve_tokens: 4096,
        });

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("run should complete without timeout");

        assert_eq!(result.status, RunStatus::Completed);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn auto_compaction_replaces_messages_before_provider_call() {
        let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
        let mut request = RunRequest::new(
            test_model(),
            vec![
                ModelMessage::system("system must stay"),
                ModelMessage::user("old context"),
                ModelMessage::user("new context"),
            ],
        );
        request.hooks = RunHooks {
            compaction: Some(Arc::new(move |messages| {
                Box::pin(async move {
                    Ok(Some(vec![
                        messages[0].clone(),
                        ModelMessage::user("<compaction_summary>\nsummary\n</compaction_summary>"),
                        messages
                            .last()
                            .cloned()
                            .expect("compaction input should have latest message"),
                    ]))
                })
            })),
            tool_result_persist: None,
        };
        request.auto_compaction = Some(AutoCompactionConfig {
            reserve_tokens: 4096,
        });

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("run should complete without timeout");
        assert_eq!(result.status, RunStatus::Completed);

        let recorded = requests.lock().expect("request lock");
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].messages.len(), 3);
        assert_eq!(recorded[0].messages[0].role, crate::types::Role::System);
        assert_eq!(recorded[0].messages[0].text(), "system must stay");
        assert!(recorded[0].messages[1]
            .text()
            .contains("<compaction_summary>"));
        assert_eq!(recorded[0].messages[2].text(), "new context");
    }

    #[tokio::test]
    async fn compaction_failure_fails_run_and_surfaces_error() {
        let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
        request.hooks = RunHooks {
            compaction: Some(Arc::new(move |_messages| {
                Box::pin(async {
                    Err(RociError::InvalidState(
                        "forced compaction failure".to_string(),
                    ))
                })
            })),
            tool_result_persist: None,
        };
        request.auto_compaction = Some(AutoCompactionConfig {
            reserve_tokens: 4096,
        });

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("run should complete without timeout");

        assert_eq!(result.status, RunStatus::Failed);
        let error = result.error.unwrap_or_default();
        assert!(error.contains("compaction failed"));
        assert!(error.contains("forced compaction failure"));
    }
}
