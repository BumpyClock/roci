use super::super::approvals::{
    ApprovalDecision, ApprovalHandler, ApprovalKind, ApprovalPolicy, ApprovalRequest,
};
use super::super::events::{AgentEvent, RunEvent, RunEventPayload, RunEventStream, RunLifecycle};
use super::super::types::{RunId, RunResult};
use super::message_events::{
    assistant_message_snapshot, emit_message_end_if_open, emit_message_start_if_needed,
};
use super::{AgentEventSink, RunEventSink};
use crate::tools::{Tool, ToolApproval, ToolApprovalKind};
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
    agent_emitter: &AgentEventEmitter,
    policy: &ApprovalPolicy,
    handler: Option<&ApprovalHandler>,
    call: &AgentToolCall,
    tool: Option<&dyn Tool>,
) -> ApprovalDecision {
    match policy {
        ApprovalPolicy::Always => ApprovalDecision::Accept,
        ApprovalPolicy::Never => ApprovalDecision::Decline,
        ApprovalPolicy::Ask => {
            let approval = tool
                .map(Tool::approval)
                .unwrap_or_else(|| ToolApproval::requires_approval(ToolApprovalKind::Other));
            let ToolApproval::RequiresApproval { kind } = approval else {
                return ApprovalDecision::Accept;
            };
            let kind = approval_kind_for_tool_metadata(kind);
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
            agent_emitter.emit(AgentEvent::Approval {
                request: request.clone(),
            });
            let decision = if let Some(handler) = handler {
                handler(request.clone()).await
            } else {
                ApprovalDecision::Decline
            };
            agent_emitter.emit(AgentEvent::ApprovalResolved {
                request_id: request.id,
                decision,
            });
            decision
        }
    }
}

pub(super) async fn resolve_iteration_limit_approval(
    emitter: &RunEventEmitter,
    agent_emitter: &AgentEventEmitter,
    handler: Option<&ApprovalHandler>,
    context: IterationLimitApprovalContext,
) -> ApprovalDecision {
    let IterationLimitApprovalContext {
        run_id,
        iteration,
        current_limit,
        extension,
        attempt,
    } = context;
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
    agent_emitter.emit(AgentEvent::Approval {
        request: request.clone(),
    });
    let decision = if let Some(handler) = handler {
        handler(request.clone()).await
    } else {
        ApprovalDecision::Decline
    };
    agent_emitter.emit(AgentEvent::ApprovalResolved {
        request_id: request.id,
        decision,
    });
    decision
}

#[derive(Debug, Clone, Copy)]
pub(super) struct IterationLimitApprovalContext {
    pub(super) run_id: RunId,
    pub(super) iteration: usize,
    pub(super) current_limit: usize,
    pub(super) extension: usize,
    pub(super) attempt: usize,
}

fn approval_kind_for_tool_metadata(kind: ToolApprovalKind) -> ApprovalKind {
    match kind {
        ToolApprovalKind::CommandExecution => ApprovalKind::CommandExecution,
        ToolApprovalKind::FileChange => ApprovalKind::FileChange,
        ToolApprovalKind::Other => ApprovalKind::Other,
    }
}

pub(super) fn approval_allows_execution(decision: ApprovalDecision) -> bool {
    matches!(
        decision,
        ApprovalDecision::Accept | ApprovalDecision::AcceptForSession
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use crate::tools::{AgentTool, AgentToolParameters};
    use uuid::Uuid;

    fn emitter_with_events() -> (RunEventEmitter, Arc<Mutex<Vec<RunEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::<RunEvent>::new()));
        let sink_events = events.clone();
        let sink: RunEventSink = Arc::new(move |event| {
            sink_events.lock().expect("event lock").push(event);
        });
        (RunEventEmitter::new(Uuid::new_v4(), Some(sink)), events)
    }

    fn tool_call(name: &str) -> AgentToolCall {
        AgentToolCall {
            id: format!("{name}-call"),
            name: name.to_string(),
            arguments: serde_json::json!({}),
            recipient: None,
        }
    }

    fn tool(name: &str, approval: ToolApproval) -> AgentTool {
        AgentTool::new(
            name,
            format!("{name} tool"),
            AgentToolParameters::empty(),
            |_args, _ctx| async { Ok(serde_json::json!({})) },
        )
        .with_approval(approval)
    }

    #[tokio::test]
    async fn ask_policy_requires_approval_for_shell_and_write_file() {
        let (emitter, events) = emitter_with_events();
        let agent_emitter = AgentEventEmitter::new(None);
        let shell = tool(
            "shell",
            ToolApproval::requires_approval(ToolApprovalKind::CommandExecution),
        );
        let write_file = tool(
            "write_file",
            ToolApproval::requires_approval(ToolApprovalKind::FileChange),
        );

        let shell_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::Ask,
            None,
            &tool_call("shell"),
            Some(&shell),
        )
        .await;
        let write_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::Ask,
            None,
            &tool_call("write_file"),
            Some(&write_file),
        )
        .await;

        assert_eq!(shell_decision, ApprovalDecision::Decline);
        assert_eq!(write_decision, ApprovalDecision::Decline);

        let events = events.lock().expect("event lock");
        let requests: Vec<(ApprovalKind, String)> = events
            .iter()
            .filter_map(|event| match &event.payload {
                RunEventPayload::ApprovalRequired { request } => Some((
                    request.kind,
                    request
                        .payload
                        .get("tool_name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )),
                _ => None,
            })
            .collect();

        assert_eq!(
            requests,
            vec![
                (ApprovalKind::CommandExecution, "shell".to_string()),
                (ApprovalKind::FileChange, "write_file".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn ask_policy_auto_accepts_explicit_safe_tools() {
        let (emitter, events) = emitter_with_events();
        let agent_emitter = AgentEventEmitter::new(None);
        let read_file = tool("read_file", ToolApproval::safe_read_only());

        let decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::Ask,
            None,
            &tool_call("read_file"),
            Some(&read_file),
        )
        .await;

        assert_eq!(decision, ApprovalDecision::Accept);
        assert!(events.lock().expect("event lock").is_empty());
    }

    #[tokio::test]
    async fn ask_policy_requires_approval_for_custom_default_and_unknown_tools() {
        let (emitter, events) = emitter_with_events();
        let agent_emitter = AgentEventEmitter::new(None);

        let custom = AgentTool::new(
            "custom_tool",
            "custom tool",
            AgentToolParameters::empty(),
            |_args, _ctx| async { Ok(serde_json::json!({})) },
        );
        let custom_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::Ask,
            None,
            &tool_call("custom_tool"),
            Some(&custom),
        )
        .await;
        let unknown_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::Ask,
            None,
            &tool_call("unknown_tool"),
            None,
        )
        .await;

        assert_eq!(custom_decision, ApprovalDecision::Decline);
        assert_eq!(unknown_decision, ApprovalDecision::Decline);

        let events = events.lock().expect("event lock");
        let approvals = events
            .iter()
            .filter(|event| matches!(event.payload, RunEventPayload::ApprovalRequired { .. }))
            .count();
        assert_eq!(approvals, 2);
    }
}
