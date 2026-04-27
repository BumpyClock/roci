use super::chat::{
    AgentRuntimeError, AgentRuntimeEventPayload, ChatProjector, MessageStatus, RuntimeCursor,
    ToolStatus, TurnStatus,
};
use crate::agent_loop::ToolUpdatePayload;
use crate::types::{AgentToolCall, AgentToolResult, ContentPart, ModelMessage, Role};

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
