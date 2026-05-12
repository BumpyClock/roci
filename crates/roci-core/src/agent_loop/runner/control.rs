use super::super::approvals::{
    ApprovalAction, ApprovalContext, ApprovalDecision, ApprovalEvaluation,
    ApprovalFilesystemAccess, ApprovalGrant, ApprovalGrantKey, ApprovalHandler, ApprovalKind,
    ApprovalPolicy, ApprovalRequest, ApprovalSafetyFloor,
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
use crate::tools::ToolFilesystemAccess;
use crate::tools::{Tool, ToolActionFloor, ToolSafetyKind, ToolSafetyPlan};
use crate::types::{AgentToolCall, ModelMessage, StreamEventType, TextStreamDelta};
use std::sync::Arc;

pub(super) struct StreamDeltaState<'a> {
    pub(super) iteration_text: &'a mut String,
    pub(super) tool_calls: &'a mut Vec<AgentToolCall>,
    pub(super) stream_done: &'a mut bool,
    pub(super) message_open: &'a mut bool,
}

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
    mut delta: TextStreamDelta,
    tools: &[Arc<dyn Tool>],
    state: StreamDeltaState<'_>,
) -> Option<String> {
    let StreamDeltaState {
        iteration_text,
        tool_calls,
        stream_done,
        message_open,
    } = state;

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
                let mut tc = tc;
                super::tooling::normalize_tool_call_alias(tools, &mut tc);
                delta.tool_call = Some(tc.clone());

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
                    assistant_message_event: delta,
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
            if let Some(reasoning) = delta.reasoning.as_ref() {
                if !reasoning.is_empty() {
                    let reasoning_text = reasoning.clone();
                    emitter.emit(
                        RunEventStream::Reasoning,
                        RunEventPayload::ReasoningDelta {
                            text: reasoning_text.clone(),
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
                        assistant_message_event: delta,
                    });
                    agent_emitter.emit(AgentEvent::Reasoning {
                        text: reasoning_text,
                    });
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
                    assistant_message_event: delta,
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
    safety_plan: &ToolSafetyPlan,
) -> ApprovalDecision {
    let tool_kind = safety_plan.approval.kind;
    let allow_session = safety_plan.approval.allow_session;
    let permission_kind = permission_kind_for_tool_metadata(tool_kind, tool.is_some());
    let session_key =
        allow_session.then(|| ToolPermissionSessionKey::for_tool_call(permission_kind, call));
    let grant_key = allow_session.then(|| {
        ApprovalGrantKey::new(
            permission_kind,
            call.name.clone(),
            call.recipient.clone(),
            Some(call.arguments.clone()),
            None,
        )
    });
    let legacy_session_hit = match &session_key {
        Some(session_key) => session_approvals.lock().await.contains(session_key),
        None => false,
    };
    let evaluation_policy = if legacy_session_hit {
        let mut policy = policy.clone();
        if let Some(grant_key) = &grant_key {
            policy.session_grants.grants.push(ApprovalGrant::Exact {
                key: grant_key.clone(),
            });
        }
        policy
    } else {
        policy.clone()
    };
    let context = ApprovalContext {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        tool_kind: Some(tool_kind),
        preview: serde_json::json!({
            "tool_name": call.name.clone(),
            "tool_call_id": call.id.clone(),
        }),
        metadata: serde_json::Value::Null,
        command: safety_plan.command.clone(),
        filesystem: safety_plan_filesystem_accesses(safety_plan),
        action_floor: approval_action_floor_for_plan(safety_plan),
        sandbox: None,
        mcp: None,
        network: None,
        grant_key,
    };
    let evaluation = evaluation_policy.evaluate(&context);

    match evaluation.action {
        ApprovalAction::Allow => {
            if allow_session && legacy_session_hit && evaluation.matched_session_grant {
                return ApprovalDecision::AcceptForSession;
            }
            return ApprovalDecision::Accept;
        }
        ApprovalAction::Deny => return ApprovalDecision::Decline,
        ApprovalAction::Ask => {}
    }

    if safety_plan.approval.auto_accept_under_ask
        && matches!(policy.default_action, ApprovalAction::Ask)
        && evaluation.matched_rules.is_empty()
        && evaluation.safety_floors.is_empty()
    {
        return ApprovalDecision::Accept;
    }

    let kind = approval_kind_for_tool_metadata(tool_kind);
    let request = ApprovalRequest {
        id: call.id.clone(),
        kind,
        allow_session,
        reason: evaluation
            .reason
            .clone()
            .or_else(|| safety_plan.approval.reason.clone())
            .or_else(|| Some(format!("Tool: {}", call.name))),
        payload: serde_json::json!({
            "tool_name": call.name.clone(),
            "tool_call_id": call.id.clone(),
            "preview": context.preview,
            "command": sanitized_command_for_payload(context.command.as_ref()),
            "filesystem": context.filesystem,
            "evaluation": sanitized_evaluation_for_payload(&evaluation),
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
            session_key.clone(),
        )
        .await
    } else if let Some(handler) = handler {
        handler(request.clone()).await
    } else {
        ApprovalDecision::Decline
    };
    let decision = if allow_session {
        decision
    } else if matches!(decision, ApprovalDecision::AcceptForSession) {
        ApprovalDecision::Accept
    } else {
        decision
    };
    if let (ApprovalDecision::AcceptForSession, Some(session_key)) = (decision, session_key) {
        session_approvals.lock().await.insert(session_key);
    }
    agent_emitter.emit(AgentEvent::ApprovalResolved {
        request_id: request.id,
        decision,
    });
    decision
}

fn safety_plan_filesystem_accesses(safety_plan: &ToolSafetyPlan) -> Vec<ApprovalFilesystemAccess> {
    safety_plan
        .filesystem
        .iter()
        .map(approval_filesystem_access)
        .collect()
}

fn approval_filesystem_access(access: &ToolFilesystemAccess) -> ApprovalFilesystemAccess {
    let path = access.path.to_string_lossy();
    ApprovalFilesystemAccess {
        operation: access.operation,
        decision: crate::security::filesystem::FilesystemDecision {
            allowed: true,
            normalized_path: Some(normalize_approval_path(&path)),
            reason: "approval context path fact".to_string(),
            matched_boundary: None,
        },
    }
}

fn approval_action_floor_for_plan(safety_plan: &ToolSafetyPlan) -> Option<ApprovalSafetyFloor> {
    let floor = safety_plan.approval.action_floor?;
    let effect = match floor {
        ToolActionFloor::Ask => ApprovalAction::Ask,
        ToolActionFloor::Deny => ApprovalAction::Deny,
    };
    Some(ApprovalSafetyFloor {
        id: "tool_action_floor".to_string(),
        effect,
        reason: safety_plan
            .approval
            .reason
            .clone()
            .unwrap_or_else(|| "tool safety plan requires approval floor".to_string()),
    })
}

fn normalize_approval_path(path: &str) -> std::path::PathBuf {
    let path = std::path::PathBuf::from(path);
    if path.is_absolute() {
        return path;
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(&path))
        .map(|path| crate::security::filesystem::lexical_normalize(&path))
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "failed to resolve cwd for approval path fact");
            crate::security::filesystem::lexical_normalize(&path)
        })
}

fn sanitized_evaluation_for_payload(evaluation: &ApprovalEvaluation) -> ApprovalEvaluation {
    let mut sanitized = evaluation.clone();
    sanitized.suggested_grant = evaluation
        .suggested_grant
        .as_ref()
        .and_then(sanitized_grant_for_payload);
    sanitized
}

fn sanitized_command_for_payload(
    command: Option<&crate::security::command::CommandInsight>,
) -> serde_json::Value {
    let Some(command) = command else {
        return serde_json::Value::Null;
    };
    serde_json::json!({
        "primary_executable": command.primary_executable.clone(),
        "categories": command.categories.clone(),
        "confidence": command.confidence.clone(),
        "reasons": command.reasons.clone(),
    })
}

fn sanitized_grant_for_payload(grant: &ApprovalGrant) -> Option<ApprovalGrant> {
    match grant {
        ApprovalGrant::Exact { key } => Some(ApprovalGrant::Exact {
            key: ApprovalGrantKey {
                permission_kind: key.permission_kind,
                tool_name: key.tool_name.clone(),
                recipient_or_server: key.recipient_or_server.clone(),
                arguments_digest: key.arguments_digest.clone(),
                tool_provided_key: None,
            },
        }),
        ApprovalGrant::Rule { .. } => None,
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
        allow_session: false,
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

fn approval_kind_for_tool_metadata(kind: ToolSafetyKind) -> ApprovalKind {
    match kind {
        ToolSafetyKind::CommandExecution => ApprovalKind::CommandExecution,
        ToolSafetyKind::FileChange => ApprovalKind::FileChange,
        ToolSafetyKind::Read
        | ToolSafetyKind::Mcp
        | ToolSafetyKind::CustomTool
        | ToolSafetyKind::Other => ApprovalKind::Other,
    }
}

fn permission_kind_for_tool_metadata(kind: ToolSafetyKind, known_tool: bool) -> ToolPermissionKind {
    match kind {
        ToolSafetyKind::CommandExecution => ToolPermissionKind::Shell,
        ToolSafetyKind::FileChange => ToolPermissionKind::Write,
        ToolSafetyKind::Read => ToolPermissionKind::Read,
        ToolSafetyKind::Mcp => ToolPermissionKind::Mcp,
        ToolSafetyKind::CustomTool => ToolPermissionKind::CustomTool,
        ToolSafetyKind::Other if known_tool => ToolPermissionKind::CustomTool,
        ToolSafetyKind::Other => ToolPermissionKind::Other,
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

    use crate::agent_loop::approvals::{ApprovalMatcher, ApprovalRule};
    use crate::human_interaction::ToolPermissionDecision;
    use crate::tools::{AgentTool, AgentToolParameters, ToolSafetySummary};
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
            called_as: None,
            recipient: None,
        }
    }

    fn safety_summary(kind: ToolSafetyKind) -> ToolSafetySummary {
        ToolSafetySummary {
            approval_kind: kind,
            ..ToolSafetySummary::default()
        }
    }

    fn tool(name: &str, plan: ToolSafetyPlan) -> AgentTool {
        let summary = safety_summary(plan.approval.kind);
        AgentTool::new(
            name,
            format!("{name} tool"),
            AgentToolParameters::empty(),
            |_args, _ctx| async { Ok(serde_json::json!({})) },
        )
        .with_static_safety(plan, summary)
    }

    fn session_approvals() -> ToolPermissionSessionApprovals {
        Arc::new(tokio::sync::Mutex::new(HashSet::new()))
    }

    fn tool_permission_request_count(events: &Arc<Mutex<Vec<AgentEvent>>>) -> usize {
        events
            .lock()
            .expect("agent event lock")
            .iter()
            .filter(|event| matches!(event, AgentEvent::HumanInteractionRequested { .. }))
            .count()
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

    async fn routed_permission_kind(tool_kind: Option<ToolSafetyKind>) -> ToolPermissionKind {
        let (emitter, _events) = emitter_with_events();
        let (agent_emitter, agent_events, coordinator) =
            responding_permission_emitter(ToolPermissionDecision::AllowOnce);
        let approvals = session_approvals();
        let call = tool_call("permission_tool");
        let safety_plan = tool_kind
            .map(ToolSafetyPlan::approval_required)
            .unwrap_or_default();
        let owned_tool = tool_kind.map(|_| tool("permission_tool", safety_plan.clone()));

        let decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::ask(),
            None,
            Some(&coordinator),
            &approvals,
            &call,
            owned_tool.as_ref().map(|tool| tool as &dyn Tool),
            &safety_plan,
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
        let shell_plan = ToolSafetyPlan::approval_required(ToolSafetyKind::CommandExecution);
        let write_plan = ToolSafetyPlan::approval_required(ToolSafetyKind::FileChange);
        let shell = tool("shell", shell_plan.clone());
        let write_file = tool("write_file", write_plan.clone());
        let approvals = session_approvals();

        let shell_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::ask(),
            None,
            None,
            &approvals,
            &tool_call("shell"),
            Some(&shell),
            &shell_plan,
        )
        .await;
        let write_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::ask(),
            None,
            None,
            &approvals,
            &tool_call("write_file"),
            Some(&write_file),
            &write_plan,
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
        for event in events.iter() {
            let RunEventPayload::ApprovalRequired { request } = &event.payload else {
                continue;
            };
            assert!(
                request.payload.get("arguments").is_none(),
                "approval payload must not expose raw tool arguments"
            );
            assert!(request.payload.get("preview").is_some());
            assert!(request.payload.get("evaluation").is_some());
        }
    }

    #[tokio::test]
    async fn always_policy_prompts_for_destructive_shell_floor() {
        let (emitter, events) = emitter_with_events();
        let agent_emitter = AgentEventEmitter::new(None);
        let shell_plan = ToolSafetyPlan::from_command_insight(
            crate::security::command::classify_shell_command("rm -rf target"),
        );
        let shell = tool("shell", shell_plan.clone());
        let approvals = session_approvals();
        let call = AgentToolCall {
            id: "shell-call".to_string(),
            name: "shell".to_string(),
            arguments: serde_json::json!({ "command": "rm -rf target" }),
            called_as: None,
            recipient: None,
        };

        let decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::always(),
            None,
            None,
            &approvals,
            &call,
            Some(&shell),
            &shell_plan,
        )
        .await;

        assert_eq!(decision, ApprovalDecision::Decline);
        let events = events.lock().expect("event lock");
        let RunEventPayload::ApprovalRequired { request } = &events[0].payload else {
            panic!("expected approval request");
        };
        assert_eq!(
            request
                .payload
                .pointer("/evaluation/action")
                .and_then(serde_json::Value::as_str),
            Some("ask")
        );
        assert_eq!(
            request
                .payload
                .pointer("/evaluation/safety_floors/0/effect")
                .and_then(serde_json::Value::as_str),
            Some("ask")
        );
    }

    #[tokio::test]
    async fn deny_action_floor_declines_without_prompt() {
        let (emitter, events) = emitter_with_events();
        let agent_emitter = AgentEventEmitter::new(None);
        let mut plan = ToolSafetyPlan::approval_required(ToolSafetyKind::CommandExecution);
        plan.approval.action_floor = Some(ToolActionFloor::Deny);
        plan.approval.reason = Some("blocked by tool safety plan".to_string());
        let shell = tool("shell", plan.clone());
        let approvals = session_approvals();

        let decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::always(),
            None,
            None,
            &approvals,
            &tool_call("shell"),
            Some(&shell),
            &plan,
        )
        .await;

        assert_eq!(decision, ApprovalDecision::Decline);
        assert!(events.lock().expect("event lock").is_empty());
    }

    #[tokio::test]
    async fn approval_payload_includes_safety_facts_and_omits_raw_arguments() {
        let (emitter, events) = emitter_with_events();
        let agent_events = Arc::new(Mutex::new(Vec::<AgentEvent>::new()));
        let sink_agent_events = agent_events.clone();
        let agent_sink: AgentEventSink = Arc::new(move |event| {
            sink_agent_events
                .lock()
                .expect("agent event lock")
                .push(event);
        });
        let agent_emitter = AgentEventEmitter::new(Some(agent_sink));
        let shell_plan = ToolSafetyPlan::from_command_insight(
            crate::security::command::classify_shell_command("rm -rf sk-secret-leak-123"),
        );
        let mut shell_plan = shell_plan;
        shell_plan.filesystem.push(ToolFilesystemAccess {
            operation: crate::security::filesystem::PathOperation::Delete,
            path: "target/tmp".into(),
        });
        let shell = tool("shell", shell_plan.clone());
        let approvals = session_approvals();
        let call = AgentToolCall {
            id: "shell-call".to_string(),
            name: "shell".to_string(),
            arguments: serde_json::json!({ "command": "rm -rf sk-secret-leak-123" }),
            called_as: None,
            recipient: None,
        };

        let decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::always(),
            None,
            None,
            &approvals,
            &call,
            Some(&shell),
            &shell_plan,
        )
        .await;

        assert_eq!(decision, ApprovalDecision::Decline);
        let events = events.lock().expect("event lock");
        let RunEventPayload::ApprovalRequired { request } = &events[0].payload else {
            panic!("expected approval request");
        };
        let payload = serde_json::to_string(&request.payload).expect("payload serializes");
        assert!(!payload.contains("sk-secret-leak-123"));
        assert!(!payload.contains("normalized_command"));
        assert!(request.payload.get("arguments").is_none());
        assert_eq!(
            request
                .payload
                .pointer("/command/primary_executable")
                .and_then(serde_json::Value::as_str),
            Some("rm")
        );
        assert!(request.payload.get("filesystem").is_some());
        assert!(request.payload.get("evaluation").is_some());

        let agent_events = agent_events.lock().expect("agent event lock");
        let agent_request = agent_events
            .iter()
            .find_map(|event| match event {
                AgentEvent::Approval { request } => Some(request),
                _ => None,
            })
            .expect("agent approval event");
        let agent_payload =
            serde_json::to_string(&agent_request.payload).expect("agent payload serializes");
        assert!(!agent_payload.contains("sk-secret-leak-123"));
        assert!(!agent_payload.contains("normalized_command"));
        assert!(agent_request.payload.get("arguments").is_none());
    }

    #[tokio::test]
    async fn grep_without_path_uses_current_directory_for_filesystem_matchers() {
        let (emitter, events) = emitter_with_events();
        let agent_emitter = AgentEventEmitter::new(None);
        let grep_plan = ToolSafetyPlan::file_search(".");
        let grep = tool("grep", grep_plan.clone());
        let approvals = session_approvals();
        let call = AgentToolCall {
            id: "grep-call".to_string(),
            name: "grep".to_string(),
            arguments: serde_json::json!({ "pattern": "needle" }),
            called_as: None,
            recipient: None,
        };
        let mut policy = ApprovalPolicy::always();
        policy.rules.push(ApprovalRule::new(
            "ask-grep-cwd",
            ApprovalAction::Ask,
            ApprovalMatcher::FilesystemPath {
                operation: crate::security::filesystem::PathOperation::Search,
                path: std::env::current_dir().expect("current dir"),
            },
        ));

        let decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &policy,
            None,
            None,
            &approvals,
            &call,
            Some(&grep),
            &grep_plan,
        )
        .await;

        assert_eq!(decision, ApprovalDecision::Decline);
        let events = events.lock().expect("event lock");
        let request_count = events
            .iter()
            .filter(|event| matches!(event.payload, RunEventPayload::ApprovalRequired { .. }))
            .count();
        assert_eq!(request_count, 1);
    }

    #[tokio::test]
    async fn filesystem_boundary_matchers_use_lexically_normalized_path_facts() {
        let (emitter, events) = emitter_with_events();
        let agent_emitter = AgentEventEmitter::new(None);
        let read_plan = ToolSafetyPlan::file_read("../secret");
        let read_file = tool("read_file", read_plan.clone());
        let approvals = session_approvals();
        let call = AgentToolCall {
            id: "read-call".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({ "path": "../secret" }),
            called_as: None,
            recipient: None,
        };
        let mut policy = ApprovalPolicy::always();
        policy.rules.push(ApprovalRule::new(
            "ask-cwd",
            ApprovalAction::Ask,
            ApprovalMatcher::FilesystemBoundary {
                operation: crate::security::filesystem::PathOperation::Read,
                boundary: crate::security::filesystem::PathBoundary::root(
                    std::env::current_dir().expect("current dir"),
                ),
            },
        ));

        let decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &policy,
            None,
            None,
            &approvals,
            &call,
            Some(&read_file),
            &read_plan,
        )
        .await;

        assert_eq!(decision, ApprovalDecision::Accept);
        assert!(events.lock().expect("event lock").is_empty());
    }

    #[tokio::test]
    async fn ask_policy_auto_accepts_explicit_safe_tools() {
        let (emitter, events) = emitter_with_events();
        let agent_emitter = AgentEventEmitter::new(None);
        let read_plan = ToolSafetyPlan::file_read("README.md");
        let read_file = tool("read_file", read_plan.clone());
        let approvals = session_approvals();

        let decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::ask(),
            None,
            None,
            &approvals,
            &tool_call("read_file"),
            Some(&read_file),
            &read_plan,
        )
        .await;

        assert_eq!(decision, ApprovalDecision::Accept);
        assert!(events.lock().expect("event lock").is_empty());
    }

    #[tokio::test]
    async fn allow_session_false_auto_accepts_and_downgrades_session_accept() {
        let (emitter, events) = emitter_with_events();
        let agent_emitter = AgentEventEmitter::new(None);
        let approvals = session_approvals();
        let host_plan = ToolSafetyPlan::host_input();
        let host_tool = tool("ask_user", host_plan.clone());

        let auto_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::ask(),
            None,
            None,
            &approvals,
            &tool_call("ask_user"),
            Some(&host_tool),
            &host_plan,
        )
        .await;

        assert_eq!(auto_decision, ApprovalDecision::Accept);
        assert!(events.lock().expect("event lock").is_empty());
        assert!(approvals.lock().await.is_empty());

        let (emitter, _events) = emitter_with_events();
        let (agent_emitter, agent_events, coordinator) =
            responding_permission_emitter(ToolPermissionDecision::AllowForSession);
        let mut policy = ApprovalPolicy::ask();
        policy.rules.push(ApprovalRule::new(
            "ask-host-input",
            ApprovalAction::Ask,
            ApprovalMatcher::ToolName {
                name: "ask_user".to_string(),
            },
        ));

        let prompted_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &policy,
            None,
            Some(&coordinator),
            &approvals,
            &tool_call("ask_user"),
            Some(&host_tool),
            &host_plan,
        )
        .await;

        assert_eq!(prompted_decision, ApprovalDecision::Accept);
        assert!(approvals.lock().await.is_empty());
        let events = agent_events.lock().expect("agent event lock");
        let permission = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::HumanInteractionRequested { request } => match &request.payload {
                    HumanInteractionPayload::ToolPermission(permission) => Some(permission),
                    _ => None,
                },
                _ => None,
            })
            .expect("tool permission request");
        assert!(!permission.approval.allow_session);
        assert!(permission.session_key.is_none());
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
        let custom_plan = ToolSafetyPlan::default();
        let unknown_plan = ToolSafetyPlan::default();
        let custom_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::ask(),
            None,
            None,
            &approvals,
            &tool_call("custom_tool"),
            Some(&custom),
            &custom_plan,
        )
        .await;
        let unknown_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::ask(),
            None,
            None,
            &approvals,
            &tool_call("unknown_tool"),
            None,
            &unknown_plan,
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
            routed_permission_kind(Some(ToolSafetyKind::CommandExecution)).await,
            ToolPermissionKind::Shell
        );
        assert_eq!(
            routed_permission_kind(Some(ToolSafetyKind::FileChange)).await,
            ToolPermissionKind::Write
        );
        assert_eq!(
            routed_permission_kind(Some(ToolSafetyKind::Read)).await,
            ToolPermissionKind::Read
        );
        assert_eq!(
            routed_permission_kind(Some(ToolSafetyKind::Mcp)).await,
            ToolPermissionKind::Mcp
        );
        assert_eq!(
            routed_permission_kind(Some(ToolSafetyKind::CustomTool)).await,
            ToolPermissionKind::CustomTool
        );
        assert_eq!(
            routed_permission_kind(Some(ToolSafetyKind::Other)).await,
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
            let shell_plan = ToolSafetyPlan::approval_required(ToolSafetyKind::CommandExecution);
            let shell = tool("shell", shell_plan.clone());
            let decision = resolve_approval(
                &emitter,
                &agent_emitter,
                &ApprovalPolicy::ask(),
                None,
                Some(&coordinator),
                &approvals,
                &tool_call("shell"),
                Some(&shell),
                &shell_plan,
            )
            .await;

            assert_eq!(decision, expected);
        }
    }

    #[tokio::test]
    async fn legacy_session_grant_reuses_only_same_final_arguments() {
        let (emitter, _events) = emitter_with_events();
        let (agent_emitter, agent_events, coordinator) =
            responding_permission_emitter(ToolPermissionDecision::AllowForSession);
        let approvals = session_approvals();
        let shell_plan = ToolSafetyPlan::approval_required(ToolSafetyKind::CommandExecution);
        let shell = tool("shell", shell_plan.clone());
        let first_call = AgentToolCall {
            id: "first".to_string(),
            name: "shell".to_string(),
            arguments: serde_json::json!({ "command": "echo one" }),
            called_as: None,
            recipient: None,
        };
        let second_call = AgentToolCall {
            id: "second".to_string(),
            name: "shell".to_string(),
            arguments: serde_json::json!({ "command": "echo one" }),
            called_as: None,
            recipient: None,
        };
        let different_args = AgentToolCall {
            id: "third".to_string(),
            name: "shell".to_string(),
            arguments: serde_json::json!({ "command": "echo two" }),
            called_as: None,
            recipient: None,
        };

        let first_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::ask(),
            None,
            Some(&coordinator),
            &approvals,
            &first_call,
            Some(&shell),
            &shell_plan,
        )
        .await;
        assert_eq!(first_decision, ApprovalDecision::AcceptForSession);
        assert_eq!(tool_permission_request_count(&agent_events), 1);

        let second_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::ask(),
            None,
            Some(&coordinator),
            &approvals,
            &second_call,
            Some(&shell),
            &shell_plan,
        )
        .await;
        assert_eq!(second_decision, ApprovalDecision::AcceptForSession);
        assert_eq!(tool_permission_request_count(&agent_events), 1);

        let different_decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &ApprovalPolicy::ask(),
            None,
            Some(&coordinator),
            &approvals,
            &different_args,
            Some(&shell),
            &shell_plan,
        )
        .await;
        assert_eq!(different_decision, ApprovalDecision::AcceptForSession);
        assert_eq!(tool_permission_request_count(&agent_events), 2);
    }

    #[tokio::test]
    async fn legacy_session_grant_does_not_override_explicit_ask_rule() {
        let (emitter, events) = emitter_with_events();
        let agent_emitter = AgentEventEmitter::new(None);
        let approvals = session_approvals();
        let call = AgentToolCall {
            id: "shell-call".to_string(),
            name: "shell".to_string(),
            arguments: serde_json::json!({ "command": "echo cached" }),
            called_as: None,
            recipient: None,
        };
        let shell_plan = ToolSafetyPlan::approval_required(ToolSafetyKind::CommandExecution);
        let shell = tool("shell", shell_plan.clone());
        approvals
            .lock()
            .await
            .insert(ToolPermissionSessionKey::for_tool_call(
                ToolPermissionKind::Shell,
                &call,
            ));
        let mut policy = ApprovalPolicy::always();
        policy.rules.push(ApprovalRule::new(
            "ask-shell",
            ApprovalAction::Ask,
            ApprovalMatcher::ToolName {
                name: "shell".to_string(),
            },
        ));

        let decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &policy,
            None,
            None,
            &approvals,
            &call,
            Some(&shell),
            &shell_plan,
        )
        .await;

        assert_eq!(decision, ApprovalDecision::Decline);
        let request_count = events
            .lock()
            .expect("event lock")
            .iter()
            .filter(|event| matches!(event.payload, RunEventPayload::ApprovalRequired { .. }))
            .count();
        assert_eq!(request_count, 1);
    }

    #[tokio::test]
    async fn legacy_session_grant_does_not_override_explicit_deny_rule() {
        let (emitter, events) = emitter_with_events();
        let agent_emitter = AgentEventEmitter::new(None);
        let approvals = session_approvals();
        let call = AgentToolCall {
            id: "shell-call".to_string(),
            name: "shell".to_string(),
            arguments: serde_json::json!({ "command": "echo cached" }),
            called_as: None,
            recipient: None,
        };
        let shell_plan = ToolSafetyPlan::approval_required(ToolSafetyKind::CommandExecution);
        let shell = tool("shell", shell_plan.clone());
        approvals
            .lock()
            .await
            .insert(ToolPermissionSessionKey::for_tool_call(
                ToolPermissionKind::Shell,
                &call,
            ));
        let mut policy = ApprovalPolicy::always();
        policy.rules.push(ApprovalRule::new(
            "deny-shell",
            ApprovalAction::Deny,
            ApprovalMatcher::ToolName {
                name: "shell".to_string(),
            },
        ));

        let decision = resolve_approval(
            &emitter,
            &agent_emitter,
            &policy,
            None,
            None,
            &approvals,
            &call,
            Some(&shell),
            &shell_plan,
        )
        .await;

        assert_eq!(decision, ApprovalDecision::Decline);
        let request_count = events
            .lock()
            .expect("event lock")
            .iter()
            .filter(|event| matches!(event.payload, RunEventPayload::ApprovalRequired { .. }))
            .count();
        assert_eq!(request_count, 0);
    }
}
