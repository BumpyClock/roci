use super::*;

#[test]
fn agent_state_equality() {
    assert_eq!(AgentState::Idle, AgentState::Idle);
    assert_ne!(AgentState::Idle, AgentState::Running);
    assert_ne!(AgentState::Running, AgentState::Aborting);
}

#[test]
fn agent_state_debug() {
    let s = format!("{:?}", AgentState::Running);
    assert_eq!(s, "Running");
}

#[test]
fn agent_state_clone_copy() {
    let s = AgentState::Idle;
    let s2 = s; // Copy
    let s3 = s.clone(); // Clone
    assert_eq!(s, s2);
    assert_eq!(s2, s3);
}

#[test]
fn queue_drain_mode_all_drains_everything() {
    let mut queue = vec![ModelMessage::user("one"), ModelMessage::user("two")];
    let drained = drain_queue(&mut queue, QueueDrainMode::All);
    assert_eq!(drained.len(), 2);
    assert!(queue.is_empty());
}

#[test]
fn queue_drain_mode_one_at_a_time_drains_incrementally() {
    let mut queue = vec![
        ModelMessage::user("one"),
        ModelMessage::user("two"),
        ModelMessage::user("three"),
    ];
    let first = drain_queue(&mut queue, QueueDrainMode::OneAtATime);
    assert_eq!(first.len(), 1);
    assert_eq!(queue.len(), 2);
    let second = drain_queue(&mut queue, QueueDrainMode::OneAtATime);
    assert_eq!(second.len(), 1);
    assert_eq!(queue.len(), 1);
}

#[test]
fn agent_snapshot_debug_and_clone() {
    let snap = AgentSnapshot {
        state: AgentState::Running,
        turn_index: 2,
        message_count: 5,
        is_streaming: true,
        last_error: Some("test error".into()),
    };
    let cloned = snap.clone();
    assert_eq!(snap, cloned);
    let debug = format!("{:?}", snap);
    assert!(debug.contains("Running"));
    assert!(debug.contains("test error"));
}
