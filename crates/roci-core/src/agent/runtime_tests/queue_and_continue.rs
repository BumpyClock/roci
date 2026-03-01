use super::support::*;
use super::*;

#[tokio::test]
async fn clear_queue_apis_and_has_queued_messages_behave_consistently() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    assert!(!agent.has_queued_messages().await);

    agent.steer("s1").await;
    assert!(agent.has_queued_messages().await);
    assert_eq!(agent.steering_queue.lock().await.len(), 1);
    assert_eq!(agent.follow_up_queue.lock().await.len(), 0);

    agent.clear_steering_queue().await;
    assert!(!agent.has_queued_messages().await);
    assert!(agent.steering_queue.lock().await.is_empty());

    agent.follow_up("f1").await;
    agent.follow_up("f2").await;
    assert!(agent.has_queued_messages().await);
    assert_eq!(agent.follow_up_queue.lock().await.len(), 2);

    agent.clear_follow_up_queue().await;
    assert!(!agent.has_queued_messages().await);
    assert!(agent.follow_up_queue.lock().await.is_empty());

    agent.steer("s2").await;
    agent.follow_up("f3").await;
    assert!(agent.has_queued_messages().await);

    agent.clear_all_queues().await;
    assert!(!agent.has_queued_messages().await);
    assert!(agent.steering_queue.lock().await.is_empty());
    assert!(agent.follow_up_queue.lock().await.is_empty());
}

#[tokio::test]
async fn clearing_queues_restores_continue_without_input_assistant_guard() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    agent
        .messages
        .lock()
        .await
        .push(ModelMessage::assistant("done"));
    agent.steer("queued steer").await;

    assert!(agent.has_queued_messages().await);
    agent.clear_all_queues().await;
    assert!(!agent.has_queued_messages().await);

    let err = agent.continue_without_input().await.unwrap_err();
    assert!(matches!(err, RociError::InvalidState(_)));
    assert_eq!(
        err.to_string(),
        "Invalid state: Cannot continue from message role: assistant"
    );
}

#[tokio::test]
async fn multiple_steers_accumulate() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    agent.steer("a").await;
    agent.steer("b").await;
    agent.steer("c").await;
    assert_eq!(agent.steering_queue.lock().await.len(), 3);
}

#[tokio::test]
async fn multiple_follow_ups_accumulate() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    agent.follow_up("x").await;
    agent.follow_up("y").await;
    assert_eq!(agent.follow_up_queue.lock().await.len(), 2);
}
