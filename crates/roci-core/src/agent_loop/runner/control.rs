use super::super::approvals::{
    ApprovalDecision, ApprovalHandler, ApprovalKind, ApprovalPolicy, ApprovalRequest,
};
use super::super::events::{AgentEvent, RunEvent, RunEventPayload, RunEventStream, RunLifecycle};
use super::super::types::{RunId, RunResult};
use super::message_events::{
    assistant_message_snapshot, emit_message_end_if_open, emit_message_start_if_needed,
};
use super::{AgentEventSink, RunEventSink};
use crate::types::{AgentToolCall, ModelMessage, StreamEventType, TextStreamDelta};

pub(super) fn emit_failed_result(
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

pub(super) fn process_stream_delta(
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
                    message: assistant_message_snapshot(iteration_text, tool_calls),
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
                        message: assistant_message_snapshot(iteration_text, tool_calls),
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
                    message: assistant_message_snapshot(iteration_text, tool_calls),
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

pub(super) struct RunEventEmitter {
    run_id: RunId,
    seq: std::sync::atomic::AtomicU64,
    sink: Option<RunEventSink>,
}

impl RunEventEmitter {
    pub(super) fn new(run_id: RunId, sink: Option<RunEventSink>) -> Self {
        Self {
            run_id,
            seq: std::sync::atomic::AtomicU64::new(1),
            sink,
        }
    }

    pub(super) fn emit(&self, stream: RunEventStream, payload: RunEventPayload) {
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
pub(super) struct AgentEventEmitter {
    sink: Option<AgentEventSink>,
}

impl AgentEventEmitter {
    pub(super) fn new(sink: Option<AgentEventSink>) -> Self {
        Self { sink }
    }

    pub(super) fn emit(&self, event: AgentEvent) {
        if let Some(sink) = &self.sink {
            (sink)(event);
        }
    }
}

pub(super) async fn resolve_approval(
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

pub(super) async fn resolve_iteration_limit_approval(
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

pub(super) fn approval_allows_execution(decision: ApprovalDecision) -> bool {
    matches!(
        decision,
        ApprovalDecision::Accept | ApprovalDecision::AcceptForSession
    )
}

pub(super) fn debug_enabled() -> bool {
    matches!(std::env::var("HOMIE_DEBUG").as_deref(), Ok("1"))
        || matches!(std::env::var("HOME_DEBUG").as_deref(), Ok("1"))
}
