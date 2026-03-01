use super::support::*;
use super::*;

#[tokio::test]
async fn snapshot_starts_with_idle_defaults() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    let snap = agent.snapshot().await;

    assert_eq!(snap.state, AgentState::Idle);
    assert_eq!(snap.turn_index, 0);
    assert_eq!(snap.message_count, 0);
    assert!(!snap.is_streaming);
    assert_eq!(snap.last_error, None);
}

#[tokio::test]
async fn watch_snapshot_returns_receiver() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    let rx1 = agent.watch_snapshot();
    let rx2 = rx1.clone();

    let snap = rx1.borrow().clone();
    assert_eq!(snap.state, AgentState::Idle);
    assert_eq!(snap.turn_index, 0);
    assert_eq!(snap.message_count, 0);
    assert!(!snap.is_streaming);
    assert_eq!(snap.last_error, None);

    assert_eq!(*rx2.borrow(), snap);
}

#[tokio::test]
async fn snapshot_reflects_queued_messages() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    assert_eq!(agent.snapshot().await.message_count, 0);

    agent
        .messages
        .lock()
        .await
        .push(ModelMessage::user("hello"));
    assert_eq!(agent.snapshot().await.message_count, 1);

    agent
        .messages
        .lock()
        .await
        .push(ModelMessage::user("follow up"));
    assert_eq!(agent.snapshot().await.message_count, 2);
}

#[tokio::test]
async fn snapshot_reflects_state_changes() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    *agent.state.lock().await = AgentState::Running;
    assert_eq!(agent.snapshot().await.state, AgentState::Running);

    *agent.state.lock().await = AgentState::Aborting;
    assert_eq!(agent.snapshot().await.state, AgentState::Aborting);
}

#[tokio::test]
async fn reset_clears_snapshot_fields() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    *agent.turn_index.lock().await = 5;
    *agent.last_error.lock().await = Some("boom".into());
    agent.messages.lock().await.push(ModelMessage::user("msg"));

    agent.reset().await;

    let snap = agent.snapshot().await;
    assert_eq!(snap.state, AgentState::Idle);
    assert_eq!(snap.turn_index, 0);
    assert_eq!(snap.message_count, 0);
    assert!(!snap.is_streaming);
    assert_eq!(snap.last_error, None);
}

#[tokio::test]
async fn watch_snapshot_notifies_on_reset() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    let mut rx = agent.watch_snapshot();

    *agent.turn_index.lock().await = 3;
    *agent.last_error.lock().await = Some("err".into());

    agent.reset().await;

    rx.changed().await.unwrap();
    let snap = rx.borrow().clone();
    assert_eq!(snap.state, AgentState::Idle);
    assert_eq!(snap.turn_index, 0);
    assert_eq!(snap.last_error, None);
}
