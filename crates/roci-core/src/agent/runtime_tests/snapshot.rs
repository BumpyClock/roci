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
async fn watch_snapshot_notifies_independent_subscribers() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    let mut rx1 = agent.watch_snapshot();
    let mut rx2 = agent.watch_snapshot();

    for rx in [&rx1, &rx2] {
        let snap = rx.borrow();
        assert_eq!(snap.state, AgentState::Idle);
        assert_eq!(snap.turn_index, 0);
        assert_eq!(snap.message_count, 0);
        assert!(!snap.is_streaming);
        assert_eq!(snap.last_error, None);
    }

    agent
        .replace_messages(vec![ModelMessage::user("new history")])
        .await
        .unwrap();

    for rx in [&mut rx1, &mut rx2] {
        tokio::time::timeout(std::time::Duration::from_secs(1), rx.changed())
            .await
            .expect("history replacement should notify every subscriber")
            .unwrap();
        assert_eq!(rx.borrow().message_count, 1);
    }
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

    agent
        .replace_messages(vec![ModelMessage::user("history to reset")])
        .await
        .unwrap();
    assert_eq!(rx.borrow_and_update().message_count, 1);

    agent.reset().await;

    tokio::time::timeout(std::time::Duration::from_secs(1), rx.changed())
        .await
        .expect("reset should publish the cleared snapshot")
        .unwrap();
    let snap = rx.borrow().clone();
    assert_eq!(snap.state, AgentState::Idle);
    assert_eq!(snap.turn_index, 0);
    assert_eq!(snap.message_count, 0);
    assert_eq!(snap.last_error, None);
}
