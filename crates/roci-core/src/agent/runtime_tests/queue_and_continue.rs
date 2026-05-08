use super::support::*;
use super::*;
use crate::attachments::{Attachment, PromptInput, SelectionAttachment};

fn queue_agent() -> AgentRuntime {
    runtime_with_streaming_model("stub", "queue")
}

#[tokio::test]
async fn clear_queue_apis_and_has_queued_messages_behave_consistently() {
    let agent = queue_agent();
    assert!(!agent.has_queued_messages().await);

    agent.steer("s1").await.expect("steer should queue");
    assert!(agent.has_queued_messages().await);
    assert_eq!(agent.steering_queue.lock().await.len(), 1);
    assert_eq!(agent.follow_up_queue.lock().await.len(), 0);

    agent.clear_steering_queue().await;
    assert!(!agent.has_queued_messages().await);
    assert!(agent.steering_queue.lock().await.is_empty());

    agent.follow_up("f1").await.expect("follow-up should queue");
    agent.follow_up("f2").await.expect("follow-up should queue");
    assert!(agent.has_queued_messages().await);
    assert_eq!(agent.follow_up_queue.lock().await.len(), 2);

    agent.clear_follow_up_queue().await;
    assert!(!agent.has_queued_messages().await);
    assert!(agent.follow_up_queue.lock().await.is_empty());

    agent.steer("s2").await.expect("steer should queue");
    agent.follow_up("f3").await.expect("follow-up should queue");
    assert!(agent.has_queued_messages().await);

    agent.clear_all_queues().await;
    assert!(!agent.has_queued_messages().await);
    assert!(agent.steering_queue.lock().await.is_empty());
    assert!(agent.follow_up_queue.lock().await.is_empty());
}

#[tokio::test]
async fn clearing_queues_restores_continue_without_input_assistant_guard() {
    let agent = queue_agent();
    agent
        .messages
        .lock()
        .await
        .push(ModelMessage::assistant("done"));
    agent
        .steer("queued steer")
        .await
        .expect("steer should queue");

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
    let agent = queue_agent();
    agent.steer("a").await.expect("steer should queue");
    agent.steer("b").await.expect("steer should queue");
    agent.steer("c").await.expect("steer should queue");
    assert_eq!(agent.steering_queue.lock().await.len(), 3);
}

#[tokio::test]
async fn multiple_follow_ups_accumulate() {
    let agent = queue_agent();
    agent.follow_up("x").await.expect("follow-up should queue");
    agent.follow_up("y").await.expect("follow-up should queue");
    assert_eq!(agent.follow_up_queue.lock().await.len(), 2);
}

#[tokio::test]
async fn steer_and_follow_up_accept_prompt_input_attachments() {
    let agent = queue_agent();

    agent
        .steer(
            PromptInput::new("steer").with_attachment(Attachment::Selection(
                SelectionAttachment::new("steer-selection-marker"),
            )),
        )
        .await
        .expect("steer should queue");
    agent
        .follow_up(
            PromptInput::new("follow").with_attachment(Attachment::Selection(
                SelectionAttachment::new("follow-selection-marker"),
            )),
        )
        .await
        .expect("follow-up should queue");

    assert!(agent.steering_queue.lock().await[0]
        .text()
        .contains("steer-selection-marker"));
    assert!(agent.follow_up_queue.lock().await[0]
        .text()
        .contains("follow-selection-marker"));
}
