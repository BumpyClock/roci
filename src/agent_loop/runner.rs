//! Runner interfaces for the agent loop.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::time::{self, Duration};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider::{self, ProviderRequest, ToolDefinition};
use crate::tools::tool::Tool;
use crate::types::{
    message::ContentPart,
    AgentToolCall,
    AgentToolResult,
    GenerationSettings,
    ModelMessage,
    StreamEventType,
};

use super::approvals::{ApprovalDecision, ApprovalHandler, ApprovalPolicy, ApprovalRequest, ApprovalKind};
use super::events::{RunEvent, RunEventPayload, RunEventStream, RunLifecycle};
use super::types::{RunId, RunResult};

/// Callback used for streaming run events.
pub type RunEventSink = Arc<dyn Fn(RunEvent) + Send + Sync>;

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
}

impl LoopRunner {
    pub fn new(config: RociConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Runner for LoopRunner {
    async fn start(&self, request: RunRequest) -> Result<RunHandle, RociError> {
        let (handle, mut abort_rx, result_tx, mut input_rx) = RunHandle::new(request.run_id);
        let config = self.config.clone();

        tokio::spawn(async move {
            if debug_enabled() {
                tracing::debug!(
                    run_id = %request.run_id,
                    model = %request.model.to_string(),
                    "roci run start"
                );
            }
            let emitter = RunEventEmitter::new(request.run_id, request.event_sink);
            emitter.emit(
                RunEventStream::Lifecycle,
                RunEventPayload::Lifecycle {
                    state: RunLifecycle::Started,
                },
            );

            let provider = match provider::create_provider(&request.model, &config) {
                Ok(provider) => provider,
                Err(err) => {
                    emitter.emit(
                        RunEventStream::Lifecycle,
                        RunEventPayload::Lifecycle {
                            state: RunLifecycle::Failed {
                                error: err.to_string(),
                            },
                        },
                    );
                    let _ = result_tx.send(RunResult::failed(err.to_string()));
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

            loop {
                iteration += 1;
                if iteration > 20 {
                    emitter.emit(
                        RunEventStream::Lifecycle,
                        RunEventPayload::Lifecycle {
                            state: RunLifecycle::Failed {
                                error: "tool loop exceeded max iterations".to_string(),
                            },
                        },
                    );
                    let _ = result_tx.send(RunResult::failed("tool loop exceeded max iterations"));
                    return;
                }

                while let Ok(message) = input_rx.try_recv() {
                    messages.push(message);
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
                        emitter.emit(
                            RunEventStream::Lifecycle,
                            RunEventPayload::Lifecycle {
                                state: RunLifecycle::Failed {
                                    error: err.to_string(),
                                },
                            },
                        );
                        let _ = result_tx.send(RunResult::failed(err.to_string()));
                        return;
                    }
                };

                let mut iteration_text = String::new();
                let mut tool_calls: HashMap<String, AgentToolCall> = HashMap::new();
                let mut stream_done = false;
                let idle_timeout_ms = request.settings.stream_idle_timeout_ms.unwrap_or(120_000);
                let mut idle_sleep = (idle_timeout_ms > 0)
                    .then(|| Box::pin(time::sleep(Duration::from_millis(idle_timeout_ms))));
                loop {
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
                        _ = idle_sleep.as_mut().unwrap(), if idle_sleep.is_some() => {
                            emitter.emit(
                                RunEventStream::Lifecycle,
                                RunEventPayload::Lifecycle {
                                    state: RunLifecycle::Failed {
                                        error: "stream idle timeout".to_string(),
                                    },
                                },
                            );
                            let _ = result_tx.send(RunResult::failed("stream idle timeout"));
                            return;
                        }
                        delta = stream.next() => {
                            let Some(delta) = delta else { break; };
                            match delta {
                                Ok(delta) => {
                                    if let Some(ref mut sleep) = idle_sleep {
                                        sleep.as_mut().reset(
                                            time::Instant::now() + Duration::from_millis(idle_timeout_ms),
                                        );
                                    }
                                    match delta.event_type {
                                        StreamEventType::ToolCallDelta => {
                                            if let Some(tc) = delta.tool_call.clone() {
                                                let is_new = !tool_calls.contains_key(&tc.id);
                                                tool_calls.insert(tc.id.clone(), tc.clone());
                                                if is_new {
                                                    emitter.emit(
                                                        RunEventStream::Tool,
                                                        RunEventPayload::ToolCallStarted { call: tc.clone() },
                                                    );
                                                } else {
                                                    emitter.emit(
                                                        RunEventStream::Tool,
                                                        RunEventPayload::ToolCallDelta { call_id: tc.id.clone(), delta: tc.arguments.clone() },
                                                    );
                                                }
                                            }
                                        }
                                        StreamEventType::Reasoning => {
                                            if let Some(reasoning) = delta.reasoning.clone() {
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
                                                    RunEventPayload::AssistantDelta { text: delta.text.clone() },
                                                );
                                            }
                                        }
                                        StreamEventType::Error => {
                                            let message = if !delta.text.is_empty() {
                                                delta.text.clone()
                                            } else {
                                                "stream error".to_string()
                                            };
                                            emitter.emit(
                                                RunEventStream::Lifecycle,
                                                RunEventPayload::Lifecycle {
                                                    state: RunLifecycle::Failed {
                                                        error: message,
                                                    },
                                                },
                                            );
                                            let _ = result_tx.send(RunResult::failed("stream error"));
                                            return;
                                        }
                                        StreamEventType::Done => {
                                            stream_done = true;
                                        }
                                        _ => {}
                                    }
                                    if stream_done {
                                        break;
                                    }
                                }
                                Err(err) => {
                                    emitter.emit(
                                        RunEventStream::Lifecycle,
                                        RunEventPayload::Lifecycle {
                                            state: RunLifecycle::Failed {
                                                error: err.to_string(),
                                            },
                                        },
                                    );
                                    let _ = result_tx.send(RunResult::failed(err.to_string()));
                                    return;
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
                    assistant_content.push(ContentPart::Text { text: iteration_text });
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
                                let args = crate::tools::arguments::ToolArguments::new(call.arguments.clone());
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

                    emitter.emit(
                        RunEventStream::Tool,
                        RunEventPayload::ToolResult { result: result.clone() },
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
            }
        });

        Ok(handle)
    }
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
        let Some(sink) = &self.sink else { return; };
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
                RunEventPayload::ApprovalRequired { request: request.clone() },
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
