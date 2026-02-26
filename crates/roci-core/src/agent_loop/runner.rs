//! Runner interfaces for the agent loop.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
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
use crate::tools::tool::Tool;
use crate::types::{
    message::ContentPart, AgentToolCall, AgentToolResult, GenerationSettings, ModelMessage,
};

use super::approvals::{ApprovalDecision, ApprovalHandler, ApprovalPolicy};
use super::compaction::estimate_context_usage;
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
    pub pre_tool_use: Option<PreToolUseHook>,
    pub post_tool_use: Option<PostToolUseHook>,
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

mod control;
mod limits;
mod message_events;
mod tooling;

use control::{
    approval_allows_execution, debug_enabled, emit_failed_result, process_stream_delta,
    resolve_approval, resolve_iteration_limit_approval, AgentEventEmitter, RunEventEmitter,
};
use limits::{is_parallel_safe_tool, RunnerLimits};
use message_events::{emit_message_end_if_open, emit_message_lifecycle};
use tooling::{
    append_skipped_tool_call, append_tool_result, apply_post_tool_use_hook, canceled_tool_result,
    declined_tool_result, emit_tool_execution_end, emit_tool_execution_start,
    execute_parallel_tool_calls, execute_tool_call, ToolExecutionOutcome,
};

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
                        let compaction_cancel_token = run_cancel_token.child_token();
                        let compaction_future =
                            compact(messages.clone(), compaction_cancel_token.clone());
                        tokio::pin!(compaction_future);
                        let compaction_result = tokio::select! {
                            _ = &mut abort_rx => {
                                run_cancel_token.cancel();
                                compaction_cancel_token.cancel();
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
                            result = &mut compaction_future => result,
                        };
                        match compaction_result {
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
                                        let canceled_result = apply_post_tool_use_hook(
                                            &request.hooks,
                                            parallel_call,
                                            canceled_tool_result(parallel_call),
                                        )
                                        .await;
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
                                    &request.hooks,
                                    &pending_parallel_calls,
                                    &agent_emitter,
                                    run_cancel_token.child_token(),
                                ) => results,
                            };
                            pending_parallel_calls.clear();
                            for parallel_outcome in parallel_results {
                                emit_tool_execution_end(
                                    &agent_emitter,
                                    &parallel_outcome.call,
                                    &parallel_outcome.result,
                                );
                                let final_result = append_tool_result(
                                    &request.hooks,
                                    &emitter,
                                    &agent_emitter,
                                    &parallel_outcome.call,
                                    parallel_outcome.result,
                                    &mut iteration_failures,
                                    &mut messages,
                                )
                                .await;
                                turn_tool_results.push(final_result);
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
                                        )
                                        .await;
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

                        let outcome = if can_execute {
                            emit_tool_execution_start(&agent_emitter, call);
                            tokio::select! {
                                _ = &mut abort_rx => {
                                    run_cancel_token.cancel();
                                    let canceled_result = apply_post_tool_use_hook(
                                        &request.hooks,
                                        call,
                                        canceled_tool_result(call),
                                    )
                                    .await;
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
                                outcome = execute_tool_call(
                                    &request.tools,
                                    &request.hooks,
                                    call,
                                    &agent_emitter,
                                    run_cancel_token.child_token(),
                                ) => outcome,
                            }
                        } else {
                            ToolExecutionOutcome {
                                call: call.clone(),
                                result: declined_tool_result(call),
                            }
                        };
                        if can_execute {
                            emit_tool_execution_end(&agent_emitter, &outcome.call, &outcome.result);
                        }

                        let final_result = append_tool_result(
                            &request.hooks,
                            &emitter,
                            &agent_emitter,
                            &outcome.call,
                            outcome.result,
                            &mut iteration_failures,
                            &mut messages,
                        )
                        .await;
                        turn_tool_results.push(final_result);

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
                                    )
                                    .await;
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
                                    let canceled_result = apply_post_tool_use_hook(
                                        &request.hooks,
                                        parallel_call,
                                        canceled_tool_result(parallel_call),
                                    )
                                    .await;
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
                                &request.hooks,
                                &pending_parallel_calls,
                                &agent_emitter,
                                run_cancel_token.child_token(),
                            ) => results,
                        };
                        pending_parallel_calls.clear();
                        for parallel_outcome in parallel_results {
                            emit_tool_execution_end(
                                &agent_emitter,
                                &parallel_outcome.call,
                                &parallel_outcome.result,
                            );
                            let final_result = append_tool_result(
                                &request.hooks,
                                &emitter,
                                &agent_emitter,
                                &parallel_outcome.call,
                                parallel_outcome.result,
                                &mut iteration_failures,
                                &mut messages,
                            )
                            .await;
                            turn_tool_results.push(final_result);
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

#[cfg(test)]
#[path = "runner/tests/mod.rs"]
mod tests;
