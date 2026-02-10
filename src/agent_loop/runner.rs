//! Runner interfaces for the agent loop.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::{self, Duration};
use uuid::Uuid;

use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider::{self, ProviderRequest, ToolDefinition};
use crate::tools::tool::Tool;
use crate::types::{
    message::ContentPart, AgentToolCall, AgentToolResult, GenerationSettings, ModelMessage,
    StreamEventType, TextStreamDelta,
};

use super::approvals::{
    ApprovalDecision, ApprovalHandler, ApprovalKind, ApprovalPolicy, ApprovalRequest,
};
use super::events::{RunEvent, RunEventPayload, RunEventStream, RunLifecycle};
use super::types::{RunId, RunResult};

/// Callback used for streaming run events.
pub type RunEventSink = Arc<dyn Fn(RunEvent) + Send + Sync>;
/// Hook to compact/prune a message history before the next provider call.
pub type CompactionHandler =
    Arc<dyn Fn(&[ModelMessage]) -> Option<Vec<ModelMessage>> + Send + Sync>;
/// Hook to redact/transform tool results before persistence or context assembly.
pub type ToolResultPersistHandler = Arc<dyn Fn(AgentToolResult) -> AgentToolResult + Send + Sync>;

#[derive(Clone, Default)]
pub struct RunHooks {
    pub compaction: Option<CompactionHandler>,
    pub tool_result_persist: Option<ToolResultPersistHandler>,
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

    pub fn queue_message(&self, message: ModelMessage) -> bool {
        if let Some(tx) = &self.input_tx {
            return tx.send(message).is_ok();
        }
        false
    }

    pub async fn wait(self) -> RunResult {
        self.result_rx
            .await
            .unwrap_or_else(|_| RunResult::canceled())
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
    pub fn new(config: RociConfig) -> Self {
        Self {
            config,
            provider_factory: Arc::new(|model, config| provider::create_provider(model, config)),
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
const RUNNER_MAX_ITERATIONS_ENV: &str = "HOMIE_ROCI_RUNNER_MAX_ITERATIONS";
const RUNNER_MAX_TOOL_FAILURES_ENV: &str = "HOMIE_ROCI_RUNNER_MAX_TOOL_FAILURES";
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

#[derive(Debug, Clone, Copy)]
struct RunnerLimits {
    max_iterations: usize,
    max_tool_failures: usize,
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
            emitter.emit(
                RunEventStream::Lifecycle,
                RunEventPayload::Lifecycle {
                    state: RunLifecycle::Started,
                },
            );

            if debug_enabled() {
                tracing::debug!(
                    run_id = %request.run_id,
                    max_iterations = limits.max_iterations,
                    max_tool_failures = limits.max_tool_failures,
                    "roci runner limits"
                );
            }

            let provider = match provider_factory(&request.model, &config) {
                Ok(provider) => provider,
                Err(err) => {
                    let _ = result_tx.send(emit_failed_result(&emitter, err.to_string()));
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

            let mut messages = request.messages.clone();
            let mut iteration = 0usize;
            let mut consecutive_failed_iterations = 0usize;

            loop {
                iteration += 1;
                if iteration > limits.max_iterations {
                    let reason = format!(
                        "tool loop exceeded max iterations (max_iterations={})",
                        limits.max_iterations
                    );
                    let _ = result_tx.send(emit_failed_result(&emitter, reason));
                    return;
                }

                while let Ok(message) = input_rx.try_recv() {
                    messages.push(message);
                }

                if let Some(compact) = request.hooks.compaction.as_ref() {
                    if let Some(compacted) = compact(&messages) {
                        messages = compacted;
                    }
                }

                let sanitized_messages =
                    provider::sanitize_messages_for_provider(&messages, provider.provider_name());
                let req = ProviderRequest {
                    messages: sanitized_messages,
                    settings: request.settings.clone(),
                    tools: tool_defs.clone(),
                    response_format: request.settings.response_format.clone(),
                };

                let mut stream = match provider.stream_text(&req).await {
                    Ok(stream) => stream,
                    Err(err) => {
                        let _ = result_tx.send(emit_failed_result(&emitter, err.to_string()));
                        return;
                    }
                };

                let mut iteration_text = String::new();
                let mut tool_calls: BTreeMap<String, AgentToolCall> = BTreeMap::new();
                let mut stream_done = false;
                let idle_timeout_ms = request.settings.stream_idle_timeout_ms.unwrap_or(120_000);
                let mut idle_sleep = (idle_timeout_ms > 0)
                    .then(|| Box::pin(time::sleep(Duration::from_millis(idle_timeout_ms))));
                loop {
                    if let Some(ref mut sleep) = idle_sleep {
                        tokio::select! {
                            _ = &mut abort_rx => {
                                emitter.emit(
                                    RunEventStream::Lifecycle,
                                    RunEventPayload::Lifecycle {
                                        state: RunLifecycle::Canceled,
                                    },
                                );
                                let _ = result_tx.send(RunResult::canceled());
                                return;
                            }
                            _ = sleep.as_mut() => {
                                let _ = result_tx.send(emit_failed_result(&emitter, "stream idle timeout"));
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
                                            delta,
                                            &mut iteration_text,
                                            &mut tool_calls,
                                            &mut stream_done,
                                        ) {
                                            let _ = result_tx.send(emit_failed_result(&emitter, reason));
                                            return;
                                        }
                                        if stream_done {
                                            break;
                                        }
                                    }
                                    Err(err) => {
                                        let _ = result_tx.send(emit_failed_result(&emitter, err.to_string()));
                                        return;
                                    }
                                }
                            }
                        }
                    } else {
                        tokio::select! {
                            _ = &mut abort_rx => {
                                emitter.emit(
                                    RunEventStream::Lifecycle,
                                    RunEventPayload::Lifecycle {
                                        state: RunLifecycle::Canceled,
                                    },
                                );
                                let _ = result_tx.send(RunResult::canceled());
                                return;
                            }
                            delta = stream.next() => {
                                let Some(delta) = delta else { break; };
                                match delta {
                                    Ok(delta) => {
                                        if let Some(reason) = process_stream_delta(
                                            &emitter,
                                            delta,
                                            &mut iteration_text,
                                            &mut tool_calls,
                                            &mut stream_done,
                                        ) {
                                            let _ = result_tx.send(emit_failed_result(&emitter, reason));
                                            return;
                                        }
                                        if stream_done {
                                            break;
                                        }
                                    }
                                    Err(err) => {
                                        let _ = result_tx.send(emit_failed_result(&emitter, err.to_string()));
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }

                if debug_enabled() {
                    tracing::debug!(
                        run_id = %request.run_id,
                        iteration,
                        stream_done,
                        tool_calls = tool_calls.len(),
                        text_len = iteration_text.len(),
                        "roci iteration complete"
                    );
                }

                if tool_calls.is_empty() {
                    emitter.emit(
                        RunEventStream::Lifecycle,
                        RunEventPayload::Lifecycle {
                            state: RunLifecycle::Completed,
                        },
                    );
                    let _ = result_tx.send(RunResult::completed());
                    if debug_enabled() {
                        tracing::debug!(run_id = %request.run_id, "roci run completed");
                    }
                    return;
                }

                let mut assistant_content: Vec<ContentPart> = Vec::new();
                if !iteration_text.is_empty() {
                    assistant_content.push(ContentPart::Text {
                        text: iteration_text,
                    });
                }
                for call in tool_calls.values() {
                    assistant_content.push(ContentPart::ToolCall(call.clone()));
                }
                messages.push(ModelMessage {
                    role: crate::types::Role::Assistant,
                    content: assistant_content,
                    name: None,
                    timestamp: Some(chrono::Utc::now()),
                });

                let mut iteration_failures = 0usize;
                for call in tool_calls.values() {
                    let decision = resolve_approval(
                        &emitter,
                        &request.approval_policy,
                        request.approval_handler.as_ref(),
                        call,
                    )
                    .await;

                    if matches!(decision, ApprovalDecision::Cancel) {
                        emitter.emit(
                            RunEventStream::Lifecycle,
                            RunEventPayload::Lifecycle {
                                state: RunLifecycle::Canceled,
                            },
                        );
                        let _ = result_tx.send(RunResult::canceled());
                        if debug_enabled() {
                            tracing::debug!(run_id = %request.run_id, "roci run canceled");
                        }
                        return;
                    }

                    let result = if matches!(
                        decision,
                        ApprovalDecision::Accept | ApprovalDecision::AcceptForSession
                    ) {
                        let tool = request.tools.iter().find(|t| t.name() == call.name);
                        match tool {
                            Some(t) => {
                                let args = crate::tools::arguments::ToolArguments::new(
                                    call.arguments.clone(),
                                );
                                let ctx = crate::tools::tool::ToolExecutionContext {
                                    metadata: serde_json::Value::Null,
                                    tool_call_id: Some(call.id.clone()),
                                    tool_name: Some(call.name.clone()),
                                };
                                match t.execute(&args, &ctx).await {
                                    Ok(val) => AgentToolResult {
                                        tool_call_id: call.id.clone(),
                                        result: val,
                                        is_error: false,
                                    },
                                    Err(e) => AgentToolResult {
                                        tool_call_id: call.id.clone(),
                                        result: serde_json::json!({ "error": e.to_string() }),
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
                    } else {
                        AgentToolResult {
                            tool_call_id: call.id.clone(),
                            result: serde_json::json!({ "error": "approval declined" }),
                            is_error: true,
                        }
                    };

                    let result = if let Some(handler) = request.hooks.tool_result_persist.as_ref() {
                        handler(result)
                    } else {
                        result
                    };

                    if result.is_error {
                        iteration_failures = iteration_failures.saturating_add(1);
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

                    messages.push(ModelMessage::tool_result(
                        result.tool_call_id.clone(),
                        result.result,
                        result.is_error,
                    ));
                }

                if iteration_failures == tool_calls.len() {
                    consecutive_failed_iterations = consecutive_failed_iterations.saturating_add(1);
                } else {
                    consecutive_failed_iterations = 0;
                }

                if consecutive_failed_iterations >= limits.max_tool_failures {
                    let reason = format!(
                        "tool call failure limit reached (max_failures={}, consecutive_failures={})",
                        limits.max_tool_failures,
                        consecutive_failed_iterations
                    );
                    let _ = result_tx.send(emit_failed_result(&emitter, reason));
                    return;
                }
            }
        });

        Ok(handle)
    }
}

fn emit_failed_result(emitter: &RunEventEmitter, reason: impl Into<String>) -> RunResult {
    let reason = reason.into();
    emitter.emit(
        RunEventStream::Lifecycle,
        RunEventPayload::Lifecycle {
            state: RunLifecycle::Failed {
                error: reason.clone(),
            },
        },
    );
    RunResult::failed(reason)
}

fn process_stream_delta(
    emitter: &RunEventEmitter,
    delta: TextStreamDelta,
    iteration_text: &mut String,
    tool_calls: &mut BTreeMap<String, AgentToolCall>,
    stream_done: &mut bool,
) -> Option<String> {
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

                let is_new = !tool_calls.contains_key(&tc.id);
                tool_calls.insert(tc.id.clone(), tc.clone());
                if is_new {
                    emitter.emit(
                        RunEventStream::Tool,
                        RunEventPayload::ToolCallStarted { call: tc },
                    );
                } else {
                    emitter.emit(
                        RunEventStream::Tool,
                        RunEventPayload::ToolCallDelta {
                            call_id: tc.id.clone(),
                            delta: tc.arguments.clone(),
                        },
                    );
                }
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
                        RunEventPayload::ReasoningDelta { text: reasoning },
                    );
                }
            }
        }
        StreamEventType::TextDelta => {
            if !delta.text.is_empty() {
                iteration_text.push_str(&delta.text);
                emitter.emit(
                    RunEventStream::Assistant,
                    RunEventPayload::AssistantDelta {
                        text: delta.text.clone(),
                    },
                );
            }
        }
        StreamEventType::Error => {
            let message = if delta.text.trim().is_empty() {
                "stream error".to_string()
            } else {
                delta.text
            };
            return Some(message);
        }
        StreamEventType::Done => {
            *stream_done = true;
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

    use crate::agent_loop::RunStatus;
    use crate::models::ModelCapabilities;
    use crate::provider::{ModelProvider, ProviderResponse};
    use crate::tools::tool::{AgentTool, ToolExecutionContext};
    use crate::tools::types::AgentToolParameters;
    use crate::types::Usage;

    #[derive(Clone, Copy)]
    enum ProviderScenario {
        MissingOptionalFields,
        RepeatedToolFailure,
    }

    struct StubProvider {
        scenario: ProviderScenario,
        calls: AtomicUsize,
        capabilities: ModelCapabilities,
    }

    impl StubProvider {
        fn new(scenario: ProviderScenario) -> Self {
            Self {
                scenario,
                calls: AtomicUsize::new(0),
                capabilities: ModelCapabilities::default(),
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
            _request: &ProviderRequest,
        ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
            let _ = self.calls.fetch_add(1, Ordering::SeqCst);
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
            };
            Ok(Box::pin(stream::iter(events)))
        }
    }

    fn test_runner(scenario: ProviderScenario) -> LoopRunner {
        let factory: ProviderFactory =
            Arc::new(move |_model, _config| Ok(Box::new(StubProvider::new(scenario))));
        LoopRunner::with_provider_factory(RociConfig::new(), factory)
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

    #[tokio::test]
    async fn no_panic_when_stream_optional_fields_missing() {
        let runner = test_runner(ProviderScenario::MissingOptionalFields);
        let (sink, _events) = capture_events();
        let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
        request.event_sink = Some(sink);

        let handle = runner.start(request).await.expect("start run");
        let result = timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("run wait timeout");
        assert_eq!(result.status, RunStatus::Completed);
    }

    #[tokio::test]
    async fn tool_failures_are_bounded_with_deterministic_reason() {
        let runner = test_runner(ProviderScenario::RepeatedToolFailure);
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
}
