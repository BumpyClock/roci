use super::chat::{
    AgentRuntimeError, AgentRuntimeEventPayload, AgentRuntimeEventStore, ApprovalStatus,
    ChatProjector, ChatRuntimeConfig, HumanInteractionStatus, JsonlAgentRuntimeEventStore,
    MessageStatus, RuntimeCursor, SessionResourceSnapshot, ThreadId, ToolStatus, TurnStatus,
};
use crate::agent_loop::{ApprovalDecision, ApprovalKind, ApprovalRequest, ToolUpdatePayload};
use crate::human_interaction::{
    AskUserRequest, HumanInteractionPayload, HumanInteractionRequest, HumanInteractionResponse,
    HumanInteractionResponsePayload, HumanInteractionSource,
};
use crate::session::{LogicalPath, SessionResourceNamespace};
use crate::tools::AskUserPrompt;
use crate::types::{AgentToolCall, AgentToolResult, ContentPart, ModelMessage, Role};
use chrono::Utc;

fn resource(
    namespace: SessionResourceNamespace,
    path: Option<&str>,
    len: u64,
) -> SessionResourceSnapshot {
    SessionResourceSnapshot {
        namespace,
        path: path.map(|path| LogicalPath::parse(path).expect("path parses")),
        len,
        updated_at: Utc::now(),
        metadata: serde_json::json!({ "origin": "projection-test" }),
    }
}

fn approval_request(id: &str) -> ApprovalRequest {
    ApprovalRequest {
        id: id.to_string(),
        kind: ApprovalKind::CommandExecution,
        allow_session: true,
        reason: Some("run shell".to_string()),
        payload: serde_json::json!({ "tool_name": "shell" }),
        suggested_policy_change: None,
    }
}

fn ask_user_request() -> HumanInteractionRequest {
    HumanInteractionRequest {
        request_id: uuid::Uuid::new_v4(),
        source: HumanInteractionSource::Host,
        payload: HumanInteractionPayload::AskUser(AskUserRequest {
            prompt: AskUserPrompt::Confirm {
                id: "continue".to_string(),
                question: "Continue?".to_string(),
                default: None,
            },
        }),
        timeout_ms: None,
        created_at: Utc::now(),
    }
}

#[test]
fn projector_defaults_to_one_thread_snapshot() {
    let projector = ChatProjector::default();
    let snapshot = projector.read_snapshot();

    assert_eq!(snapshot.schema_version, 1);
    assert_eq!(snapshot.threads.len(), 1);
    assert_eq!(snapshot.threads[0].thread_id, projector.default_thread_id());
    assert_eq!(snapshot.threads[0].last_seq, 0);
}

#[test]
fn queue_turn_emits_monotonic_seq_after_state_update() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(vec![
        ModelMessage::user("hello"),
        ModelMessage::assistant("hi"),
    ]);
    let thread = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");

    assert_eq!(
        queued
            .events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    assert_eq!(thread.last_seq, 5);
    assert_eq!(thread.turns.len(), 1);
    assert_eq!(thread.messages.len(), 2);

    match &queued.events[0].payload {
        AgentRuntimeEventPayload::TurnQueued { turn } => {
            assert_eq!(turn.turn_id, queued.turn_id);
            assert_eq!(turn.status, TurnStatus::Queued);
        }
        other => panic!("expected turn queued, got {other:?}"),
    }
    match &queued.events[4].payload {
        AgentRuntimeEventPayload::MessageCompleted { message } => {
            assert_eq!(message.message_id, thread.messages[1].message_id);
            assert_eq!(message.status, MessageStatus::Completed);
        }
        other => panic!("expected message completed, got {other:?}"),
    }
}

#[test]
fn message_lifecycle_projects_started_updated_and_completed_snapshots() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(Vec::new());
    projector.start_turn(queued.turn_id).expect("turn starts");

    let message = projector
        .start_message(queued.turn_id, ModelMessage::assistant(""))
        .expect("message starts");
    let updated = projector
        .update_message(message.message_id, ModelMessage::assistant("hel"))
        .expect("message updates");
    let completed = projector
        .complete_message(message.message_id, ModelMessage::assistant("hello"))
        .expect("message completes");
    let thread = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");
    let projected = thread
        .messages
        .iter()
        .find(|candidate| candidate.message_id == message.message_id)
        .expect("message projected");

    assert_eq!(projected.status, MessageStatus::Completed);
    assert_eq!(projected.payload.text(), "hello");
    assert!(projected.completed_at.is_some());

    match updated.payload {
        AgentRuntimeEventPayload::MessageUpdated { message } => {
            assert_eq!(message.payload.text(), "hel");
        }
        other => panic!("expected message updated, got {other:?}"),
    }
    match completed.payload {
        AgentRuntimeEventPayload::MessageCompleted { message } => {
            assert_eq!(message.status, MessageStatus::Completed);
            assert_eq!(message.completed_at, projected.completed_at);
        }
        other => panic!("expected message completed, got {other:?}"),
    }
}

#[test]
fn tool_lifecycle_projects_started_updated_and_completed_snapshots() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(Vec::new());
    projector.start_turn(queued.turn_id).expect("turn starts");

    let started = projector
        .start_tool(
            queued.turn_id,
            "call-1",
            "search",
            serde_json::json!({ "query": "roci" }),
        )
        .expect("tool starts");
    let partial = ToolUpdatePayload {
        content: vec![ContentPart::Text {
            text: "partial".to_string(),
        }],
        details: serde_json::json!({ "seen": 1 }),
    };
    let updated = projector
        .update_tool(queued.turn_id, "call-1", partial.clone())
        .expect("tool updates");
    let result = AgentToolResult {
        tool_call_id: "call-1".to_string(),
        result: serde_json::json!({ "ok": true }),
        is_error: false,
    };
    let completed = projector
        .complete_tool(queued.turn_id, "call-1", result.clone())
        .expect("tool completes");
    let thread = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");
    let projected = thread
        .tools
        .iter()
        .find(|candidate| candidate.tool_call_id == "call-1")
        .expect("tool projected");

    assert!(matches!(
        started.payload,
        AgentRuntimeEventPayload::ToolStarted { .. }
    ));
    assert_eq!(projected.status, ToolStatus::Completed);
    assert_eq!(projected.partial_result, Some(partial));
    assert_eq!(projected.final_result, Some(result));
    assert!(projected.completed_at.is_some());
    assert!(!thread.turns[0]
        .active_tool_call_ids
        .contains(&"call-1".to_string()));

    match updated.payload {
        AgentRuntimeEventPayload::ToolUpdated { tool } => {
            assert_eq!(tool.status, ToolStatus::Running);
            assert!(tool.partial_result.is_some());
        }
        other => panic!("expected tool updated, got {other:?}"),
    }
    match completed.payload {
        AgentRuntimeEventPayload::ToolCompleted { tool } => {
            assert_eq!(tool.status, ToolStatus::Completed);
            assert!(tool.final_result.is_some());
        }
        other => panic!("expected tool completed, got {other:?}"),
    }
}

#[test]
fn approval_lifecycle_projects_required_resolved_and_canceled_snapshots() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(Vec::new());
    projector.start_turn(queued.turn_id).expect("turn starts");
    let request = ApprovalRequest {
        id: "approval-1".to_string(),
        kind: ApprovalKind::CommandExecution,
        allow_session: true,
        reason: Some("run shell".to_string()),
        payload: serde_json::json!({ "tool_name": "shell" }),
        suggested_policy_change: None,
    };

    let required = projector
        .require_approval(queued.turn_id, request.clone())
        .expect("approval required projects");
    let resolved = projector
        .resolve_approval(queued.turn_id, &request.id, ApprovalDecision::Accept)
        .expect("approval resolved projects");
    let cancel_request = ApprovalRequest {
        id: "approval-2".to_string(),
        ..request
    };
    projector
        .require_approval(queued.turn_id, cancel_request.clone())
        .expect("second approval required projects");
    let canceled = projector
        .cancel_approval(queued.turn_id, &cancel_request.id)
        .expect("approval canceled projects");
    let thread = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");

    assert_eq!(thread.approvals.len(), 2);
    assert_eq!(thread.approvals[0].status, ApprovalStatus::Resolved);
    assert_eq!(thread.approvals[0].decision, Some(ApprovalDecision::Accept));
    assert_eq!(thread.approvals[1].status, ApprovalStatus::Canceled);
    assert_eq!(thread.approvals[1].decision, Some(ApprovalDecision::Cancel));

    assert!(matches!(
        required.payload,
        AgentRuntimeEventPayload::ApprovalRequired { .. }
    ));
    assert!(matches!(
        resolved.payload,
        AgentRuntimeEventPayload::ApprovalResolved { .. }
    ));
    assert!(matches!(
        canceled.payload,
        AgentRuntimeEventPayload::ApprovalCanceled { .. }
    ));
}

#[test]
fn cancel_turn_marks_pending_approvals_canceled() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(Vec::new());
    projector.start_turn(queued.turn_id).expect("turn starts");
    let request = ApprovalRequest {
        id: "approval-1".to_string(),
        kind: ApprovalKind::CommandExecution,
        allow_session: true,
        reason: Some("run shell".to_string()),
        payload: serde_json::json!({ "tool_name": "shell" }),
        suggested_policy_change: None,
    };
    projector
        .require_approval(queued.turn_id, request)
        .expect("approval required projects");

    let approval_canceled = projector
        .cancel_pending_approvals(queued.turn_id)
        .expect("pending approvals cancel");
    let turn_canceled = projector.cancel_turn(queued.turn_id).expect("turn cancels");
    let thread = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");

    assert!(matches!(
        approval_canceled[0].payload,
        AgentRuntimeEventPayload::ApprovalCanceled { .. }
    ));
    assert!(matches!(
        turn_canceled.payload,
        AgentRuntimeEventPayload::TurnCanceled { .. }
    ));
    assert_eq!(thread.approvals[0].status, ApprovalStatus::Canceled);
    assert_eq!(thread.approvals[0].decision, Some(ApprovalDecision::Cancel));
}

#[test]
fn human_interaction_lifecycle_projects_requested_resolved_and_canceled_snapshots() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(Vec::new());
    projector.start_turn(queued.turn_id).expect("turn starts");
    let request = HumanInteractionRequest {
        request_id: uuid::Uuid::new_v4(),
        source: HumanInteractionSource::ModelTool {
            tool_call_id: "ask-user-call-1".to_string(),
            tool_name: "ask_user".to_string(),
        },
        payload: HumanInteractionPayload::AskUser(AskUserRequest {
            prompt: AskUserPrompt::Question {
                id: "unit".to_string(),
                question: "C or F?".to_string(),
                placeholder: None,
                default: None,
                multiline: false,
            },
        }),
        timeout_ms: Some(1_000),
        created_at: Utc::now(),
    };

    let requested = projector
        .request_human_interaction(queued.turn_id, request.clone())
        .expect("human interaction request projects");
    let resolved = projector
        .resolve_human_interaction(
            queued.turn_id,
            HumanInteractionResponse {
                request_id: request.request_id,
                payload: HumanInteractionResponsePayload::Declined,
                resolved_at: Utc::now(),
            },
        )
        .expect("human interaction response projects");
    let cancel_request = HumanInteractionRequest {
        request_id: uuid::Uuid::new_v4(),
        ..request
    };
    projector
        .request_human_interaction(queued.turn_id, cancel_request.clone())
        .expect("second human interaction request projects");
    let canceled = projector
        .cancel_human_interaction(
            queued.turn_id,
            cancel_request.request_id,
            Some("turn canceled".to_string()),
        )
        .expect("human interaction cancel projects");
    let thread = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");

    assert_eq!(thread.human_interactions.len(), 2);
    assert_eq!(
        thread.human_interactions[0].status,
        HumanInteractionStatus::Resolved
    );
    assert!(thread.human_interactions[0].response.is_some());
    assert_eq!(
        thread.human_interactions[1].status,
        HumanInteractionStatus::Canceled
    );
    assert_eq!(
        thread.human_interactions[1].error.as_deref(),
        Some("turn canceled")
    );

    assert!(matches!(
        requested.payload,
        AgentRuntimeEventPayload::HumanInteractionRequested { .. }
    ));
    assert!(matches!(
        resolved.payload,
        AgentRuntimeEventPayload::HumanInteractionResolved { .. }
    ));
    assert!(matches!(
        canceled.payload,
        AgentRuntimeEventPayload::HumanInteractionCanceled { .. }
    ));
}

#[test]
fn cancel_turn_marks_pending_human_interactions_canceled() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(Vec::new());
    projector.start_turn(queued.turn_id).expect("turn starts");
    let request = HumanInteractionRequest {
        request_id: uuid::Uuid::new_v4(),
        source: HumanInteractionSource::Host,
        payload: HumanInteractionPayload::AskUser(AskUserRequest {
            prompt: AskUserPrompt::Confirm {
                id: "continue".to_string(),
                question: "Continue?".to_string(),
                default: None,
            },
        }),
        timeout_ms: None,
        created_at: Utc::now(),
    };
    projector
        .request_human_interaction(queued.turn_id, request)
        .expect("human interaction request projects");

    let interaction_canceled = projector
        .cancel_pending_human_interactions(queued.turn_id)
        .expect("pending human interactions cancel");
    let turn_canceled = projector.cancel_turn(queued.turn_id).expect("turn cancels");
    let thread = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");

    assert!(matches!(
        interaction_canceled[0].payload,
        AgentRuntimeEventPayload::HumanInteractionCanceled { .. }
    ));
    assert!(matches!(
        turn_canceled.payload,
        AgentRuntimeEventPayload::TurnCanceled { .. }
    ));
    assert_eq!(
        thread.human_interactions[0].status,
        HumanInteractionStatus::Canceled
    );
}

#[test]
fn reasoning_plan_and_diff_project_latest_turn_state() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(Vec::new());
    projector.start_turn(queued.turn_id).expect("turn starts");
    let message = projector
        .start_message(queued.turn_id, ModelMessage::assistant(""))
        .expect("message starts");

    let first_reasoning = projector
        .update_reasoning(queued.turn_id, Some(message.message_id), "think ")
        .expect("reasoning projects");
    let second_reasoning = projector
        .update_reasoning(queued.turn_id, Some(message.message_id), "more")
        .expect("reasoning appends");
    let plan = projector
        .update_plan(queued.turn_id, "1. inspect\n2. edit")
        .expect("plan projects");
    let diff = projector
        .update_diff(queued.turn_id, "-old\n+new")
        .expect("diff projects");
    let thread = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");

    assert_eq!(thread.reasoning.len(), 1);
    assert_eq!(thread.reasoning[0].message_id, Some(message.message_id));
    assert_eq!(thread.reasoning[0].text, "think more");
    assert_eq!(thread.plans[0].plan, "1. inspect\n2. edit");
    assert_eq!(thread.diffs[0].diff, "-old\n+new");

    match first_reasoning.payload {
        AgentRuntimeEventPayload::ReasoningUpdated { reasoning, delta } => {
            assert_eq!(reasoning.text, "think ");
            assert_eq!(delta, "think ");
        }
        other => panic!("expected reasoning updated, got {other:?}"),
    }
    match second_reasoning.payload {
        AgentRuntimeEventPayload::ReasoningUpdated { reasoning, delta } => {
            assert_eq!(reasoning.text, "think more");
            assert_eq!(delta, "more");
        }
        other => panic!("expected reasoning updated, got {other:?}"),
    }
    assert!(matches!(
        plan.payload,
        AgentRuntimeEventPayload::PlanUpdated { .. }
    ));
    assert!(matches!(
        diff.payload,
        AgentRuntimeEventPayload::DiffUpdated { .. }
    ));
}

#[test]
fn record_session_resource_records_event_and_updates_snapshot_resources() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(Vec::new());
    projector.start_turn(queued.turn_id).expect("turn starts");

    let plan = resource(SessionResourceNamespace::Plan, None, 10);
    let workspace = resource(SessionResourceNamespace::Workspace, None, 20);
    let artifact = resource(
        SessionResourceNamespace::Artifacts,
        Some("reports/out.md"),
        30,
    );
    let temp_file = resource(
        SessionResourceNamespace::Temp,
        Some("scratch/cache.bin"),
        40,
    );
    let checkpoint = resource(
        SessionResourceNamespace::Checkpoints,
        Some("turn-1/state.json"),
        50,
    );
    let file = resource(SessionResourceNamespace::Files, Some("notes/today.md"), 60);
    let deleted_file = resource(SessionResourceNamespace::Files, Some("notes/today.md"), 0);

    let plan_event = projector
        .record_session_resource(
            queued.turn_id,
            AgentRuntimeEventPayload::PlanWritten {
                resource: plan.clone(),
            },
        )
        .expect("plan resource projects");
    projector
        .record_session_resource(
            queued.turn_id,
            AgentRuntimeEventPayload::WorkspaceUpdated {
                resource: workspace.clone(),
            },
        )
        .expect("workspace resource projects");
    projector
        .record_session_resource(
            queued.turn_id,
            AgentRuntimeEventPayload::ArtifactCreated {
                resource: artifact.clone(),
            },
        )
        .expect("artifact resource projects");
    projector
        .record_session_resource(
            queued.turn_id,
            AgentRuntimeEventPayload::TempFileWritten {
                resource: temp_file.clone(),
            },
        )
        .expect("temp resource projects");
    projector
        .record_session_resource(
            queued.turn_id,
            AgentRuntimeEventPayload::CheckpointCreated {
                resource: checkpoint.clone(),
            },
        )
        .expect("checkpoint resource projects");
    projector
        .record_session_resource(
            queued.turn_id,
            AgentRuntimeEventPayload::SessionFileWritten {
                resource: file.clone(),
            },
        )
        .expect("session file resource projects");
    let delete_event = projector
        .record_session_resource(
            queued.turn_id,
            AgentRuntimeEventPayload::SessionFileDeleted {
                resource: deleted_file,
            },
        )
        .expect("session file delete projects");

    let thread = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");

    assert!(matches!(
        plan_event.payload,
        AgentRuntimeEventPayload::PlanWritten { .. }
    ));
    assert!(matches!(
        delete_event.payload,
        AgentRuntimeEventPayload::SessionFileDeleted { .. }
    ));
    assert_eq!(thread.resources.plan, Some(plan));
    assert_eq!(thread.resources.workspace, Some(workspace));
    assert_eq!(thread.resources.artifacts, vec![artifact]);
    assert_eq!(thread.resources.temp_files, vec![temp_file]);
    assert_eq!(thread.resources.checkpoints, vec![checkpoint]);
    assert!(thread.resources.files.is_empty());
    assert_eq!(thread.last_seq, delete_event.seq);
}

#[test]
fn record_session_resource_rejects_wrong_namespace_without_mutation() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(Vec::new());
    projector.start_turn(queued.turn_id).expect("turn starts");
    let before = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");
    let invalid = resource(
        SessionResourceNamespace::Files,
        Some("reports/wrong-namespace.md"),
        10,
    );

    let err = projector
        .record_session_resource(
            queued.turn_id,
            AgentRuntimeEventPayload::ArtifactCreated { resource: invalid },
        )
        .expect_err("wrong namespace rejected");
    let after = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");

    assert!(matches!(err, AgentRuntimeError::ProjectionFailed { .. }));
    assert!(err.to_string().contains("artifact_created"));
    assert!(err.to_string().contains("namespace mismatch"));
    assert_eq!(after.resources, before.resources);
    assert_eq!(after.last_seq, before.last_seq);
}

#[test]
fn record_session_resource_rejects_missing_path_without_mutation() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(Vec::new());
    projector.start_turn(queued.turn_id).expect("turn starts");
    let before = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");
    let invalid = resource(SessionResourceNamespace::Temp, None, 10);

    let err = projector
        .record_session_resource(
            queued.turn_id,
            AgentRuntimeEventPayload::TempFileWritten { resource: invalid },
        )
        .expect_err("missing path rejected");
    let after = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");

    assert!(matches!(err, AgentRuntimeError::ProjectionFailed { .. }));
    assert!(err.to_string().contains("temp_file_written"));
    assert!(err.to_string().contains("path is required"));
    assert_eq!(after.resources, before.resources);
    assert_eq!(after.last_seq, before.last_seq);
}

#[test]
fn projector_events_do_not_emit_snapshot_updated_payloads() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(vec![ModelMessage::user("hello")]);

    for event in queued.events {
        let payload = serde_json::to_value(event.payload).expect("payload serializes");
        assert_ne!(payload["type"], "snapshot_updated");
    }
}

#[test]
fn bootstrap_thread_projects_exact_history_as_completed_messages() {
    let mut projector = ChatProjector::default();
    let initial_revision = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists")
        .revision;
    let history = vec![
        ModelMessage {
            role: Role::System,
            content: vec![ContentPart::Text {
                text: "stay concise".to_string(),
            }],
            name: Some("policy".to_string()),
            timestamp: None,
            metadata: None,
        },
        ModelMessage::user("find docs"),
        ModelMessage {
            role: Role::Assistant,
            content: vec![
                ContentPart::Text {
                    text: "checking".to_string(),
                },
                ContentPart::ToolCall(AgentToolCall {
                    id: "call-1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({ "query": "roci projector" }),
                    recipient: Some("web".to_string()),
                }),
            ],
            name: None,
            timestamp: None,
            metadata: None,
        },
        ModelMessage::tool_result("call-1", serde_json::json!({ "ok": true }), false),
    ];

    let snapshot = projector
        .bootstrap_thread(history.clone())
        .expect("history bootstraps");
    let read_thread = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");
    let runtime_snapshot = projector.read_snapshot();

    assert_eq!(
        snapshot
            .messages
            .iter()
            .map(|message| message.payload.clone())
            .collect::<Vec<_>>(),
        history
    );
    assert_eq!(read_thread.messages, snapshot.messages);
    assert_eq!(runtime_snapshot.threads[0].messages, snapshot.messages);
    assert_eq!(snapshot.revision, initial_revision + 1);
    assert_eq!(snapshot.turns.len(), 1);
    assert_eq!(snapshot.turns[0].turn_id.revision(), snapshot.revision);
    assert_eq!(snapshot.turns[0].status, TurnStatus::Completed);
    assert_eq!(
        snapshot.turns[0].message_ids,
        snapshot
            .messages
            .iter()
            .map(|message| message.message_id)
            .collect::<Vec<_>>()
    );
    assert!(snapshot.active_turn_id.is_none());
    assert!(snapshot.tools.is_empty());
    assert!(snapshot.messages.iter().all(|message| {
        message.message_id.revision() == snapshot.revision
            && message.status == MessageStatus::Completed
            && message.completed_at.is_some()
    }));
}

#[test]
fn bootstrap_thread_does_not_replay_imported_history_as_semantic_events() {
    let mut projector = ChatProjector::default();
    let _ = projector.queue_turn(vec![ModelMessage::user("before import")]);
    let snapshot = projector
        .bootstrap_thread(vec![
            ModelMessage::user("imported prompt"),
            ModelMessage::assistant("imported answer"),
        ])
        .expect("history bootstraps");
    let queued = projector.queue_turn(vec![ModelMessage::user("fresh prompt")]);

    let replayed = projector
        .events_after(RuntimeCursor::new(
            projector.default_thread_id(),
            snapshot.last_seq,
        ))
        .expect("fresh cursor replays");

    assert_eq!(replayed, queued.events);
    let replayed_payloads = replayed
        .iter()
        .map(|event| serde_json::to_string(&event.payload).expect("payload serializes"))
        .collect::<Vec<_>>();
    assert!(replayed_payloads
        .iter()
        .any(|payload| payload.contains("fresh prompt")));
    assert!(replayed_payloads
        .iter()
        .all(|payload| !payload.contains("imported")));
}

#[test]
fn bootstrap_thread_invalidates_prior_replay_cursors() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(vec![ModelMessage::user("before import")]);
    let old_cursor = queued.events[0].cursor();

    let snapshot = projector
        .bootstrap_thread(vec![ModelMessage::user("imported")])
        .expect("history bootstraps");
    let err = projector
        .events_after(old_cursor)
        .expect_err("old cursor is stale after bootstrap");

    assert_eq!(
        err,
        AgentRuntimeError::StaleRuntime {
            thread_id: projector.default_thread_id(),
            requested_seq: old_cursor.seq,
            oldest_available_seq: snapshot.last_seq + 1,
            latest_seq: snapshot.last_seq,
        }
    );
}

#[test]
fn terminal_methods_reject_already_terminal_turns() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(Vec::new());
    let canceled = projector.cancel_turn(queued.turn_id).expect("turn cancels");

    match canceled.payload {
        AgentRuntimeEventPayload::TurnCanceled { turn } => {
            assert_eq!(turn.status, TurnStatus::Canceled);
            assert_eq!(
                turn.completed_at,
                projector.read_thread(turn.thread_id).unwrap().turns[0].completed_at
            );
        }
        other => panic!("expected turn canceled, got {other:?}"),
    }

    let err = projector
        .start_turn(queued.turn_id)
        .expect_err("canceled turn cannot start");
    assert_eq!(
        err,
        AgentRuntimeError::AlreadyTerminal {
            turn_id: queued.turn_id,
            status: TurnStatus::Canceled,
        }
    );

    let err = projector
        .complete_turn(queued.turn_id)
        .expect_err("canceled turn cannot complete");
    assert_eq!(
        err,
        AgentRuntimeError::AlreadyTerminal {
            turn_id: queued.turn_id,
            status: TurnStatus::Canceled,
        }
    );
}

#[test]
fn projector_replays_runtime_events_into_snapshot() {
    let mut projector = ChatProjector::new(ChatRuntimeConfig::default());
    let thread_id = projector.default_thread_id();
    let queued = projector.queue_turn(vec![ModelMessage::user("hello replay")]);
    let started = projector.start_turn(queued.turn_id).unwrap();
    let message = projector
        .start_message(queued.turn_id, ModelMessage::assistant(""))
        .unwrap();
    let updated = projector
        .update_message(message.message_id, ModelMessage::assistant("stream"))
        .unwrap();
    let completed_message = projector
        .complete_message(message.message_id, ModelMessage::assistant("done"))
        .unwrap();
    let tool_started = projector
        .start_tool(
            queued.turn_id,
            "call-replay",
            "search",
            serde_json::json!({ "query": "roci" }),
        )
        .unwrap();
    let tool_updated = projector
        .update_tool(
            queued.turn_id,
            "call-replay",
            ToolUpdatePayload {
                content: vec![ContentPart::Text {
                    text: "partial".to_string(),
                }],
                details: serde_json::json!({ "seen": 1 }),
            },
        )
        .unwrap();
    let tool_completed = projector
        .complete_tool(
            queued.turn_id,
            "call-replay",
            AgentToolResult {
                tool_call_id: "call-replay".to_string(),
                result: serde_json::json!({ "ok": true }),
                is_error: false,
            },
        )
        .unwrap();
    let approval_required = projector
        .require_approval(queued.turn_id, approval_request("approval-replay"))
        .unwrap();
    let approval_resolved = projector
        .resolve_approval(queued.turn_id, "approval-replay", ApprovalDecision::Accept)
        .unwrap();
    let interaction_request = ask_user_request();
    let interaction_requested = projector
        .request_human_interaction(queued.turn_id, interaction_request.clone())
        .unwrap();
    let interaction_resolved = projector
        .resolve_human_interaction(
            queued.turn_id,
            HumanInteractionResponse {
                request_id: interaction_request.request_id,
                payload: HumanInteractionResponsePayload::Declined,
                resolved_at: Utc::now(),
            },
        )
        .unwrap();
    let reasoning = projector
        .update_reasoning(queued.turn_id, Some(message.message_id), "think")
        .unwrap();
    let plan = projector.update_plan(queued.turn_id, "plan").unwrap();
    let diff = projector.update_diff(queued.turn_id, "diff").unwrap();
    let artifact = projector
        .record_session_resource(
            queued.turn_id,
            AgentRuntimeEventPayload::ArtifactCreated {
                resource: resource(
                    SessionResourceNamespace::Artifacts,
                    Some("replay/out.md"),
                    12,
                ),
            },
        )
        .unwrap();
    let completed = projector.complete_turn(queued.turn_id).unwrap();
    let mut events = queued.events;
    events.extend([
        started,
        message.event,
        updated,
        completed_message,
        tool_started,
        tool_updated,
        tool_completed,
        approval_required,
        approval_resolved,
        interaction_requested,
        interaction_resolved,
        reasoning,
        plan,
        diff,
        artifact,
        completed,
    ]);

    let replayed = ChatProjector::from_events(
        ChatRuntimeConfig {
            default_thread_id: Some(thread_id),
            ..ChatRuntimeConfig::default()
        },
        events.clone(),
    )
    .unwrap();
    let thread = replayed.read_thread(thread_id).unwrap();

    assert_eq!(thread.last_seq, events.last().unwrap().seq);
    assert_eq!(thread.turns[0].status, TurnStatus::Completed);
    assert_eq!(thread.messages[0].payload.text(), "hello replay");
    assert_eq!(thread.messages[1].payload.text(), "done");
    assert_eq!(thread.tools[0].status, ToolStatus::Completed);
    assert_eq!(thread.approvals[0].status, ApprovalStatus::Resolved);
    assert_eq!(
        thread.human_interactions[0].status,
        HumanInteractionStatus::Resolved
    );
    assert_eq!(thread.reasoning[0].text, "think");
    assert_eq!(thread.plans[0].plan, "plan");
    assert_eq!(thread.diffs[0].diff, "diff");
    assert_eq!(thread.resources.artifacts.len(), 1);
    assert!(thread.active_turn_id.is_none());
    assert_eq!(
        replayed
            .events_after(RuntimeCursor::new(thread_id, 0))
            .unwrap(),
        events
    );
}

#[test]
fn projector_replays_non_default_thread_events() {
    let non_default = ThreadId::new();
    let mut source = ChatProjector::new(ChatRuntimeConfig {
        default_thread_id: Some(non_default),
        ..ChatRuntimeConfig::default()
    });
    let queued = source.queue_turn(vec![ModelMessage::user("other thread")]);

    let replayed = ChatProjector::from_events(ChatRuntimeConfig::default(), queued.events).unwrap();

    assert_eq!(
        replayed.read_thread(non_default).unwrap().messages[0]
            .payload
            .text(),
        "other thread"
    );
    assert_eq!(replayed.thread_ids(), {
        let mut ids = vec![replayed.default_thread_id(), non_default];
        ids.sort_by_key(|id| id.to_string());
        ids
    });
}

#[tokio::test]
async fn all_events_orders_by_thread_seq_not_timestamp() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlAgentRuntimeEventStore::open(dir.path().join("events.jsonl")).unwrap();
    let mut projector = ChatProjector::new(ChatRuntimeConfig::default());
    let queued = projector.queue_turn(vec![ModelMessage::user("timestamp order")]);
    let mut first = queued.events[0].clone();
    let mut second = queued.events[1].clone();
    first.timestamp = Utc::now();
    second.timestamp = first.timestamp - chrono::Duration::seconds(60);
    store.append(first).await.unwrap();
    store.append(second).await.unwrap();

    let events = store.all_events().await;

    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[test]
fn normalize_for_resume_cancels_queued_and_running_turns() {
    let mut projector = ChatProjector::default();
    let running = projector.queue_turn(Vec::new());
    projector.start_turn(running.turn_id).unwrap();
    let queued = projector.queue_turn(Vec::new());

    let events = projector.normalize_for_resume().unwrap();
    let thread = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");

    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, AgentRuntimeEventPayload::TurnCanceled { .. }))
            .count(),
        2
    );
    assert_eq!(
        thread
            .turns
            .iter()
            .find(|turn| turn.turn_id == running.turn_id)
            .unwrap()
            .status,
        TurnStatus::Canceled
    );
    assert_eq!(
        thread
            .turns
            .iter()
            .find(|turn| turn.turn_id == queued.turn_id)
            .unwrap()
            .status,
        TurnStatus::Canceled
    );
    assert!(thread.active_turn_id.is_none());
}

#[test]
fn normalize_for_resume_cancels_pending_approval_and_human_interaction() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(Vec::new());
    projector.start_turn(queued.turn_id).unwrap();
    projector
        .require_approval(queued.turn_id, approval_request("approval-normalize"))
        .unwrap();
    projector
        .request_human_interaction(queued.turn_id, ask_user_request())
        .unwrap();

    let events = projector.normalize_for_resume().unwrap();
    let thread = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");

    assert!(events.iter().any(|event| matches!(
        event.payload,
        AgentRuntimeEventPayload::ApprovalCanceled { .. }
    )));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        AgentRuntimeEventPayload::HumanInteractionCanceled { .. }
    )));
    assert_eq!(thread.approvals[0].status, ApprovalStatus::Canceled);
    assert_eq!(
        thread.human_interactions[0].status,
        HumanInteractionStatus::Canceled
    );
}

#[test]
fn normalize_for_resume_finishes_active_tool_with_error_result() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(Vec::new());
    projector.start_turn(queued.turn_id).unwrap();
    projector
        .start_tool(
            queued.turn_id,
            "call-normalize",
            "search",
            serde_json::json!({ "query": "roci" }),
        )
        .unwrap();

    let events = projector.normalize_for_resume().unwrap();
    let thread = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");

    let completed = events
        .iter()
        .find_map(|event| match &event.payload {
            AgentRuntimeEventPayload::ToolCompleted { tool } => Some(tool),
            _ => None,
        })
        .expect("tool completion emitted");
    assert_eq!(completed.status, ToolStatus::Completed);
    assert!(completed.final_result.as_ref().unwrap().is_error);
    assert_eq!(thread.tools[0].status, ToolStatus::Completed);
    assert!(thread.turns[0].active_tool_call_ids.is_empty());
}

#[test]
fn normalize_for_resume_completes_streaming_message_with_current_payload() {
    let mut projector = ChatProjector::default();
    let queued = projector.queue_turn(Vec::new());
    projector.start_turn(queued.turn_id).unwrap();
    let message = projector
        .start_message(queued.turn_id, ModelMessage::assistant(""))
        .unwrap();
    projector
        .update_message(message.message_id, ModelMessage::assistant("partial"))
        .unwrap();

    let events = projector.normalize_for_resume().unwrap();
    let thread = projector
        .read_thread(projector.default_thread_id())
        .expect("default thread exists");

    let completed = events
        .iter()
        .find_map(|event| match &event.payload {
            AgentRuntimeEventPayload::MessageCompleted { message } => Some(message),
            _ => None,
        })
        .expect("message completion emitted");
    assert_eq!(completed.payload.text(), "partial");
    assert_eq!(completed.status, MessageStatus::Completed);
    assert_eq!(thread.messages[0].status, MessageStatus::Completed);
    assert_eq!(thread.messages[0].payload.text(), "partial");
}
