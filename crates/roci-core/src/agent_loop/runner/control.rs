use super::super::approvals::{
    ApprovalDecision, ApprovalHandler, ApprovalKind, ApprovalPolicy, ApprovalRequest,
};
use super::super::events::{AgentEvent, RunEvent, RunEventPayload, RunEventStream, RunLifecycle};
use super::super::types::{RunId, RunResult};
use super::message_events::{
    assistant_message_snapshot, emit_message_end_if_open, emit_message_start_if_needed,
};
use super::{AgentEventSink, RunEventSink};
use crate::human_interaction::{
    HumanInteractionCoordinator, HumanInteractionError, HumanInteractionPayload,
    HumanInteractionRequest, HumanInteractionResponse, HumanInteractionResponsePayload,
    HumanInteractionSource, ToolPermissionKind, ToolPermissionRequest, ToolPermissionResponse,
    ToolPermissionSessionApprovals, ToolPermissionSessionKey,
};
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

#[allow(clippy::too_many_arguments)]
pub(super) async fn resolve_approval(
    emitter: &RunEventEmitter,
    agent_emitter: &AgentEventEmitter,
    policy: &ApprovalPolicy,
    handler: Option<&ApprovalHandler>,
    coordinator: Option<&std::sync::Arc<HumanInteractionCoordinator>>,
    session_approvals: &ToolPermissionSessionApprovals,
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
            let ToolApproval::RequiresApproval { kind: tool_kind } = approval else {
                return ApprovalDecision::Accept;
            };
            let kind = approval_kind_for_tool_metadata(tool_kind);
            let permission_kind = permission_kind_for_tool_metadata(tool_kind, tool.is_some());
            let session_key = ToolPermissionSessionKey::for_tool_call(permission_kind, call);
            if session_approvals.lock().await.contains(&session_key) {
                return ApprovalDecision::AcceptForSession;
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
            agent_emitter.emit(AgentEvent::Approval {
                request: request.clone(),
            });
            let decision = if let Some(coordinator) = coordinator {
                resolve_tool_permission(
                    coordinator,
                    agent_emitter,
                    call,
                    request.clone(),
                    permission_kind,
                    Some(session_key.clone()),
                )
                .await
            } else if let Some(handler) = handler {
                handler(request.clone()).await
            } else {
                ApprovalDecision::Decline
            };
            if matches!(decision, ApprovalDecision::AcceptForSession) {
                session_approvals.lock().await.insert(session_key);
            }
            agent_emitter.emit(AgentEvent::ApprovalResolved {
                request_id: request.id,
                decision,
            });
            decision
        }
    }
}

async fn resolve_tool_permission(
    coordinator: &HumanInteractionCoordinator,
    agent_emitter: &AgentEventEmitter,
    call: &AgentToolCall,
    approval: ApprovalRequest,
    kind: ToolPermissionKind,
    session_key: Option<ToolPermissionSessionKey>,
) -> ApprovalDecision {
    let request_id = uuid::Uuid::new_v4();
    let request = HumanInteractionRequest {
        request_id,
        source: HumanInteractionSource::ToolPermission {
            tool_call_id: Some(call.id.clone()),
            tool_name: call.name.clone(),
        },
        payload: HumanInteractionPayload::ToolPermission(ToolPermissionRequest {
            approval,
            kind,
            tool_call_id: Some(call.id.clone()),
            tool_name: call.name.clone(),
            arguments: call.arguments.clone(),
            session_key,
        }),
        timeout_ms: None,
        created_at: chrono::Utc::now(),
    };
    let pending = match coordinator
        .create_tool_permission_request(request.clone())
        .await
    {
        Ok(pending) => pending,
        Err(error) => {
            agent_emitter.emit(AgentEvent::HumanInteractionCanceled {
                request_id,
                reason: Some(error.to_string()),
            });
            return ApprovalDecision::Decline;
        }
    };
    agent_emitter.emit(AgentEvent::HumanInteractionRequested { request });

    match pending.wait_tool_permission(None).await {
        Ok(decision) => {
            let response = HumanInteractionResponse {
                request_id,
                payload: HumanInteractionResponsePayload::ToolPermission(ToolPermissionResponse {
                    decision,
                }),
                resolved_at: chrono::Utc::now(),
            };
            agent_emitter.emit(AgentEvent::HumanInteractionResolved { response });
            ApprovalDecision::from(decision)
        }
        Err(HumanInteractionError::Canceled { .. }) => {
            agent_emitter.emit(AgentEvent::HumanInteractionCanceled {
                request_id,
                reason: Some("tool permission canceled".to_string()),
            });
            ApprovalDecision::Cancel
        }
        Err(error) => {
            agent_emitter.emit(AgentEvent::HumanInteractionCanceled {
                request_id,
                reason: Some(error.to_string()),
            });
            ApprovalDecision::Decline
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
        ToolApprovalKind::Read
        | ToolApprovalKind::Mcp
        | ToolApprovalKind::CustomTool
        | ToolApprovalKind::Other => ApprovalKind::Other,
    }
}

fn permission_kind_for_tool_metadata(
    kind: ToolApprovalKind,
    known_tool: bool,
) -> ToolPermissionKind {
    match kind {
        ToolApprovalKind::CommandExecution => ToolPermissionKind::Shell,
        ToolApprovalKind::FileChange => ToolPermissionKind::Write,
        ToolApprovalKind::Read => ToolPermissionKind::Read,
        ToolApprovalKind::Mcp => ToolPermissionKind::Mcp,
        ToolApprovalKind::CustomTool => ToolPermissionKind::CustomTool,
        ToolApprovalKind::Other if known_tool => ToolPermissionKind::CustomTool,
        ToolApprovalKind::Other => ToolPermissionKind::Other,
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

    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    use crate::human_interaction::ToolPermissionDecision;
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

    fn session_approvals() -> ToolPermissionSessionApprovals {
        Arc::new(tokio::sync::Mutex::new(HashSet::new()))
    }

    fn responding_permission_emitter(
        decision: ToolPermissionDecision,
    ) -> (
        AgentEventEmitter,
        Arc<Mutex<Vec<AgentEvent>>>,
        Arc<HumanInteractionCoordinator>,
    ) {
        let events = Arc::new(Mutex::new(Vec::<AgentEvent>::new()));
        let coordinator = Arc::new(HumanInteractionCoordinator::new());
        let sink_events = events.clone();
        let sink_coordinator = coordinator.clone();
        let sink: AgentEventSink = Arc::new(move |event| {
            if let AgentEvent::HumanInteractionRequested { request } = &event {
                let coordinator = sink_coordinator.clone();
                let request_id = request.request_id;
                tokio::spawn(async move {
                    let _ = coordinator
                        .submit_tool_permission_response(request_id, decision)
                        .await;
                });
            }
            sink_events.lock().expect("agent event lock").push(event);
        });
        (
            AgentEventEmitter::new(Some(sink)),
            events,
            coordinator.clone(),
        )
    }

    async fn routed_permission_kind(tool_kind: Option<ToolApprovalKind>) -> ToolPermissionKind {
        let (emitter, _events) = emitter_with_events();
        let (agent_emitter, agent_events, coordinator) =
            responding_permission_emitter(ToolPermissionDecision::AllowOnce);
        let approvals = session_approvals();
        let call = tool_call("permission_tool");
        let owned_tool =
            tool_kind.map(|kind| tool("permission_tool", ToolApproval::requires_approval(kind)));

        let decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::Ask,
            None,
            Some(&coordinator),
            &approvals,
            &call,
            owned_tool.as_ref().map(|tool| tool as &dyn Tool),
        )
        .await;
        assert_eq!(decision, ApprovalDecision::Accept);

        let events = agent_events.lock().expect("agent event lock");
        events
            .iter()
            .find_map(|event| match event {
                AgentEvent::HumanInteractionRequested { request } => match &request.payload {
                    HumanInteractionPayload::ToolPermission(permission) => Some(permission.kind),
                    _ => None,
                },
                _ => None,
            })
            .expect("tool permission request")
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
        let approvals = session_approvals();

        let shell_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::Ask,
            None,
            None,
            &approvals,
            &tool_call("shell"),
            Some(&shell),
        )
        .await;
        let write_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::Ask,
            None,
            None,
            &approvals,
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
        let approvals = session_approvals();

        let decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::Ask,
            None,
            None,
            &approvals,
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
        let approvals = session_approvals();

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
            None,
            &approvals,
            &tool_call("custom_tool"),
            Some(&custom),
        )
        .await;
        let unknown_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::Ask,
            None,
            None,
            &approvals,
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

    #[tokio::test]
    async fn tool_permission_requests_cover_all_permission_kinds() {
        assert_eq!(
            routed_permission_kind(Some(ToolApprovalKind::CommandExecution)).await,
            ToolPermissionKind::Shell
        );
        assert_eq!(
            routed_permission_kind(Some(ToolApprovalKind::FileChange)).await,
            ToolPermissionKind::Write
        );
        assert_eq!(
            routed_permission_kind(Some(ToolApprovalKind::Read)).await,
            ToolPermissionKind::Read
        );
        assert_eq!(
            routed_permission_kind(Some(ToolApprovalKind::Mcp)).await,
            ToolPermissionKind::Mcp
        );
        assert_eq!(
            routed_permission_kind(Some(ToolApprovalKind::CustomTool)).await,
            ToolPermissionKind::CustomTool
        );
        assert_eq!(
            routed_permission_kind(Some(ToolApprovalKind::Other)).await,
            ToolPermissionKind::CustomTool
        );
        assert_eq!(
            routed_permission_kind(None).await,
            ToolPermissionKind::Other
        );
    }

    #[tokio::test]
    async fn tool_permission_decisions_map_to_approval_decisions() {
        let cases = [
            (ToolPermissionDecision::AllowOnce, ApprovalDecision::Accept),
            (
                ToolPermissionDecision::AllowForSession,
                ApprovalDecision::AcceptForSession,
            ),
            (ToolPermissionDecision::Deny, ApprovalDecision::Decline),
            (ToolPermissionDecision::Cancel, ApprovalDecision::Cancel),
        ];

        for (permission_decision, expected) in cases {
            let (emitter, _events) = emitter_with_events();
            let (agent_emitter, _agent_events, coordinator) =
                responding_permission_emitter(permission_decision);
            let approvals = session_approvals();
            let shell = tool(
                "shell",
                ToolApproval::requires_approval(ToolApprovalKind::CommandExecution),
            );
            let decision = resolve_approval(
                &emitter,
                &agent_emitter,
                &ApprovalPolicy::Ask,
                None,
                Some(&coordinator),
                &approvals,
                &tool_call("shell"),
                Some(&shell),
            )
            .await;

            assert_eq!(decision, expected);
        }
    }

    #[tokio::test]
    async fn allow_for_session_reuses_exact_permission_key_only() {
        let (emitter, _events) = emitter_with_events();
        let (agent_emitter, agent_events, coordinator) =
            responding_permission_emitter(ToolPermissionDecision::AllowForSession);
        let approvals = session_approvals();
        let shell = tool(
            "shell",
            ToolApproval::requires_approval(ToolApprovalKind::CommandExecution),
        );
        let first_call = AgentToolCall {
            id: "first".to_string(),
            name: "shell".to_string(),
            arguments: serde_json::json!({ "command": "echo one" }),
            recipient: None,
        };
        let second_call = AgentToolCall {
            id: "second".to_string(),
            name: "shell".to_string(),
            arguments: serde_json::json!({ "command": "echo one" }),
            recipient: None,
        };
        let different_args = AgentToolCall {
            id: "third".to_string(),
            name: "shell".to_string(),
            arguments: serde_json::json!({ "command": "echo two" }),
            recipient: None,
        };

        for call in [&first_call, &second_call, &different_args] {
            let decision = resolve_approval(
                &emitter,
                &agent_emitter,
                &ApprovalPolicy::Ask,
                None,
                Some(&coordinator),
                &approvals,
                call,
                Some(&shell),
            )
            .await;
            assert_eq!(decision, ApprovalDecision::AcceptForSession);
        }

        let request_count = agent_events
            .lock()
            .expect("agent event lock")
            .iter()
            .filter(|event| matches!(event, AgentEvent::HumanInteractionRequested { .. }))
            .count();
        assert_eq!(request_count, 2);
    }
}
