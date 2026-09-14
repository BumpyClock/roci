use super::chat::{
    AgentRuntimeError, AgentRuntimeEvent, AgentRuntimeEventPayload, ChatRuntimeConfig, MessageId,
    MessageStatus, RuntimeCursor, SessionResourceSnapshot, ThreadId, TurnId, TurnSnapshot,
    TurnStatus,
};
use crate::session::{LogicalPath, SessionResourceNamespace};

fn test_resource(namespace: SessionResourceNamespace) -> SessionResourceSnapshot {
    SessionResourceSnapshot {
        namespace,
        path: Some(LogicalPath::parse("notes/today.md").expect("path parses")),
        len: 42,
        updated_at: chrono::Utc::now(),
        metadata: serde_json::json!({ "source": "test" }),
    }
}

fn test_turn(thread_id: ThreadId) -> TurnSnapshot {
    let now = chrono::Utc::now();
    TurnSnapshot {
        turn_id: TurnId::new(thread_id, 0, 1),
        thread_id,
        status: TurnStatus::Queued,
        message_ids: Vec::new(),
        active_tool_call_ids: Vec::new(),
        error: None,
        queued_at: now,
        started_at: None,
        completed_at: None,
    }
}

#[test]
fn chat_runtime_config_defaults_to_bounded_in_memory_replay() {
    let config = ChatRuntimeConfig::default();

    assert_eq!(config.replay_capacity, 512);
    assert!(config.event_store.is_none());
}

#[test]
fn runtime_cursor_is_thread_scoped() {
    let thread_id = ThreadId::nil();
    let cursor = RuntimeCursor::new(thread_id, 42);

    assert_eq!(cursor.thread_id, thread_id);
    assert_eq!(cursor.seq, 42);
}

#[test]
fn turn_and_message_ids_carry_thread_revision() {
    let thread_id = ThreadId::new();
    let turn_id = TurnId::new(thread_id, 7, 3);
    let message_id = MessageId::new(thread_id, 7, 9);

    assert_eq!(turn_id.thread_id(), thread_id);
    assert_eq!(turn_id.revision(), 7);
    assert_eq!(turn_id.ordinal(), 3);
    assert_eq!(message_id.thread_id(), thread_id);
    assert_eq!(message_id.revision(), 7);
    assert_eq!(message_id.ordinal(), 9);
}

#[test]
fn stale_runtime_error_reports_requested_and_oldest_seq() {
    let thread_id = ThreadId::nil();
    let err = AgentRuntimeError::StaleRuntime {
        thread_id,
        requested_seq: 4,
        oldest_available_seq: 12,
        latest_seq: 19,
    };
    let display = err.to_string();

    assert!(display.contains("requested seq 4"));
    assert!(display.contains("oldest available 12"));
    assert!(display.contains("latest seq 19"));
}

#[test]
fn semantic_payloads_serialize_with_stable_tags() {
    use super::chat::{
        ApprovalSnapshot, ApprovalStatus, DiffSnapshot, MessageSnapshot, PlanSnapshot,
        ReasoningSnapshot, ToolExecutionSnapshot, ToolStatus,
    };
    use crate::agent_loop::{ApprovalKind, ApprovalRequest};
    use crate::types::ModelMessage;

    let thread_id = ThreadId::nil();
    let turn = test_turn(thread_id);
    let turn_id = turn.turn_id;
    let now = turn.queued_at;
    let message = MessageSnapshot {
        message_id: MessageId::new(thread_id, 0, 1),
        thread_id,
        turn_id,
        status: MessageStatus::Completed,
        payload: ModelMessage::assistant("answer"),
        created_at: now,
        completed_at: Some(now),
    };
    let tool = ToolExecutionSnapshot {
        tool_call_id: "call-1".into(),
        thread_id,
        turn_id,
        tool_name: "read".into(),
        args: serde_json::json!({"path": "notes.txt"}),
        status: ToolStatus::Running,
        partial_result: None,
        final_result: None,
        started_at: now,
        completed_at: None,
    };
    let approval = ApprovalSnapshot {
        request: ApprovalRequest {
            id: "approval-1".into(),
            kind: ApprovalKind::Other,
            allow_session: true,
            reason: None,
            payload: serde_json::json!({}),
            suggested_policy_change: None,
        },
        thread_id,
        turn_id,
        status: ApprovalStatus::Pending,
        decision: None,
        requested_at: now,
        resolved_at: None,
    };
    let cases = [
        (
            AgentRuntimeEventPayload::TurnQueued { turn: turn.clone() },
            "turn_queued",
        ),
        (
            AgentRuntimeEventPayload::TurnStarted { turn: turn.clone() },
            "turn_started",
        ),
        (
            AgentRuntimeEventPayload::TurnCompleted { turn: turn.clone() },
            "turn_completed",
        ),
        (
            AgentRuntimeEventPayload::TurnFailed {
                turn: turn.clone(),
                error: "failure".into(),
            },
            "turn_failed",
        ),
        (
            AgentRuntimeEventPayload::TurnCanceled { turn: turn.clone() },
            "turn_canceled",
        ),
        (
            AgentRuntimeEventPayload::MessageStarted {
                message: message.clone(),
            },
            "message_started",
        ),
        (
            AgentRuntimeEventPayload::MessageUpdated {
                message: message.clone(),
            },
            "message_updated",
        ),
        (
            AgentRuntimeEventPayload::MessageCompleted {
                message: message.clone(),
            },
            "message_completed",
        ),
        (
            AgentRuntimeEventPayload::ToolStarted { tool: tool.clone() },
            "tool_started",
        ),
        (
            AgentRuntimeEventPayload::ToolUpdated { tool: tool.clone() },
            "tool_updated",
        ),
        (
            AgentRuntimeEventPayload::ToolCompleted { tool: tool.clone() },
            "tool_completed",
        ),
        (
            AgentRuntimeEventPayload::ApprovalRequired {
                approval: approval.clone(),
            },
            "approval_required",
        ),
        (
            AgentRuntimeEventPayload::ApprovalResolved {
                approval: approval.clone(),
            },
            "approval_resolved",
        ),
        (
            AgentRuntimeEventPayload::ApprovalCanceled {
                approval: approval.clone(),
            },
            "approval_canceled",
        ),
        (
            AgentRuntimeEventPayload::ReasoningUpdated {
                reasoning: ReasoningSnapshot {
                    thread_id,
                    turn_id,
                    message_id: Some(message.message_id),
                    text: "considering".into(),
                    updated_at: now,
                },
                delta: "considering".into(),
            },
            "reasoning_updated",
        ),
        (
            AgentRuntimeEventPayload::PlanUpdated {
                plan: PlanSnapshot {
                    thread_id,
                    turn_id,
                    plan: "read notes".into(),
                    updated_at: now,
                },
            },
            "plan_updated",
        ),
        (
            AgentRuntimeEventPayload::DiffUpdated {
                diff: DiffSnapshot {
                    thread_id,
                    turn_id,
                    diff: "+note".into(),
                    updated_at: now,
                },
            },
            "diff_updated",
        ),
    ];
    for (payload, expected) in cases {
        let encoded = serde_json::to_value(&payload).expect("payload serializes");
        assert_eq!(encoded["type"], expected);
        let decoded: AgentRuntimeEventPayload =
            serde_json::from_value(encoded).expect("payload deserializes");
        assert_eq!(decoded, payload);
    }
}

#[test]
fn resource_event_payloads_have_stable_names() {
    let cases = [
        (
            AgentRuntimeEventPayload::PlanWritten {
                resource: test_resource(SessionResourceNamespace::Plan),
            },
            "plan_written",
        ),
        (
            AgentRuntimeEventPayload::WorkspaceUpdated {
                resource: test_resource(SessionResourceNamespace::Workspace),
            },
            "workspace_updated",
        ),
        (
            AgentRuntimeEventPayload::ArtifactCreated {
                resource: test_resource(SessionResourceNamespace::Artifacts),
            },
            "artifact_created",
        ),
        (
            AgentRuntimeEventPayload::TempFileWritten {
                resource: test_resource(SessionResourceNamespace::Temp),
            },
            "temp_file_written",
        ),
        (
            AgentRuntimeEventPayload::CheckpointCreated {
                resource: test_resource(SessionResourceNamespace::Checkpoints),
            },
            "checkpoint_created",
        ),
        (
            AgentRuntimeEventPayload::SessionFileWritten {
                resource: test_resource(SessionResourceNamespace::Files),
            },
            "session_file_written",
        ),
        (
            AgentRuntimeEventPayload::SessionFileDeleted {
                resource: test_resource(SessionResourceNamespace::Files),
            },
            "session_file_deleted",
        ),
    ];

    for (payload, expected) in cases {
        let encoded = serde_json::to_value(payload).expect("payload serializes");

        assert_eq!(encoded["type"], expected);
    }
}

#[test]
fn event_envelope_sets_schema_and_cursor() {
    let thread_id = ThreadId::nil();
    let turn = test_turn(thread_id);
    let event = AgentRuntimeEvent::new(
        12,
        thread_id,
        Some(turn.turn_id),
        AgentRuntimeEventPayload::TurnQueued { turn },
    );

    assert_eq!(event.schema_version, 1);
    assert_eq!(event.cursor(), RuntimeCursor::new(thread_id, 12));
}

#[test]
fn message_status_contract_excludes_independent_failure_state() {
    let streaming = serde_json::to_value(MessageStatus::Streaming).unwrap();
    let completed = serde_json::to_value(MessageStatus::Completed).unwrap();

    assert_eq!(streaming, "streaming");
    assert_eq!(completed, "completed");
}
