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
fn semantic_payload_set_matches_target_contract() {
    let payload_names = [
        AgentRuntimeEventPayload::turn_queued_name(),
        AgentRuntimeEventPayload::turn_started_name(),
        AgentRuntimeEventPayload::message_started_name(),
        AgentRuntimeEventPayload::message_updated_name(),
        AgentRuntimeEventPayload::message_completed_name(),
        AgentRuntimeEventPayload::tool_started_name(),
        AgentRuntimeEventPayload::tool_updated_name(),
        AgentRuntimeEventPayload::tool_completed_name(),
        AgentRuntimeEventPayload::approval_required_name(),
        AgentRuntimeEventPayload::approval_resolved_name(),
        AgentRuntimeEventPayload::approval_canceled_name(),
        AgentRuntimeEventPayload::reasoning_updated_name(),
        AgentRuntimeEventPayload::plan_updated_name(),
        AgentRuntimeEventPayload::diff_updated_name(),
        AgentRuntimeEventPayload::plan_written_name(),
        AgentRuntimeEventPayload::workspace_updated_name(),
        AgentRuntimeEventPayload::artifact_created_name(),
        AgentRuntimeEventPayload::temp_file_written_name(),
        AgentRuntimeEventPayload::checkpoint_created_name(),
        AgentRuntimeEventPayload::session_file_written_name(),
        AgentRuntimeEventPayload::session_file_deleted_name(),
        AgentRuntimeEventPayload::turn_completed_name(),
        AgentRuntimeEventPayload::turn_failed_name(),
        AgentRuntimeEventPayload::turn_canceled_name(),
    ];

    assert_eq!(
        payload_names,
        [
            "turn_queued",
            "turn_started",
            "message_started",
            "message_updated",
            "message_completed",
            "tool_started",
            "tool_updated",
            "tool_completed",
            "approval_required",
            "approval_resolved",
            "approval_canceled",
            "reasoning_updated",
            "plan_updated",
            "diff_updated",
            "plan_written",
            "workspace_updated",
            "artifact_created",
            "temp_file_written",
            "checkpoint_created",
            "session_file_written",
            "session_file_deleted",
            "turn_completed",
            "turn_failed",
            "turn_canceled",
        ]
    );
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
