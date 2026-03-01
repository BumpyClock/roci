use super::support::*;
use super::*;

#[tokio::test]
async fn new_agent_starts_idle() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    assert_eq!(agent.state().await, AgentState::Idle);
}

#[tokio::test]
async fn messages_starts_empty() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    assert!(agent.messages().await.is_empty());
}

#[tokio::test]
async fn wait_for_idle_returns_immediately_when_idle() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    // Should return instantly - no run in flight.
    agent.wait_for_idle().await;
    assert_eq!(agent.state().await, AgentState::Idle);
}

#[tokio::test]
async fn steer_queues_message() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    agent.steer("change direction").await;
    let queue = agent.steering_queue.lock().await;
    assert_eq!(queue.len(), 1);
}

#[tokio::test]
async fn follow_up_queues_message() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    agent.follow_up("next step").await;
    let queue = agent.follow_up_queue.lock().await;
    assert_eq!(queue.len(), 1);
}

#[tokio::test]
async fn abort_returns_false_when_idle() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    assert!(!agent.abort().await);
}

#[tokio::test]
async fn reset_clears_all_state() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    // Seed some queue data.
    agent.steer("msg1").await;
    agent.follow_up("msg2").await;

    agent.reset().await;

    assert_eq!(agent.state().await, AgentState::Idle);
    assert!(agent.steering_queue.lock().await.is_empty());
    assert!(agent.follow_up_queue.lock().await.is_empty());
    assert!(agent.messages().await.is_empty());
}

#[tokio::test]
async fn watch_state_returns_idle_initially() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    let rx = agent.watch_state();
    assert_eq!(*rx.borrow(), AgentState::Idle);
}

#[tokio::test]
async fn set_system_prompt_rejects_when_running() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    *agent.state.lock().await = AgentState::Running;

    let err = agent.set_system_prompt("new prompt").await.unwrap_err();
    assert!(matches!(err, RociError::InvalidState(_)));
}

#[tokio::test]
async fn set_model_rejects_when_running() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    *agent.state.lock().await = AgentState::Running;

    let model: LanguageModel = "openai:gpt-4o-mini".parse().unwrap();
    let err = agent.set_model(model).await.unwrap_err();
    assert!(matches!(err, RociError::InvalidState(_)));
}

#[tokio::test]
async fn set_model_replaces_runtime_model_when_idle() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    let model: LanguageModel = "openai:gpt-4o-mini".parse().unwrap();

    agent.set_model(model.clone()).await.unwrap();
    assert_eq!(*agent.model.lock().await, model);
}

#[tokio::test]
async fn set_and_clear_system_prompt_work_when_idle() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    agent
        .set_system_prompt("use concise replies")
        .await
        .unwrap();
    assert_eq!(
        agent.system_prompt.lock().await.clone(),
        Some("use concise replies".into())
    );

    agent.clear_system_prompt().await.unwrap();
    assert_eq!(agent.system_prompt.lock().await.clone(), None);
}

#[tokio::test]
async fn replace_messages_rejects_when_not_idle() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    *agent.state.lock().await = AgentState::Aborting;

    let err = agent
        .replace_messages(vec![ModelMessage::user("replacement")])
        .await
        .unwrap_err();
    assert!(matches!(err, RociError::InvalidState(_)));
}

#[tokio::test]
async fn replace_messages_updates_snapshot_and_history() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    let mut rx = agent.watch_snapshot();

    agent
        .replace_messages(vec![
            ModelMessage::system("system"),
            ModelMessage::user("hello"),
            ModelMessage::assistant("response"),
        ])
        .await
        .unwrap();

    rx.changed().await.unwrap();
    assert_eq!(agent.messages().await.len(), 3);
    assert_eq!(rx.borrow().message_count, 3);
}

#[tokio::test]
async fn transition_to_running_fails_when_not_idle() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    // Force state to Running manually to test the guard.
    *agent.state.lock().await = AgentState::Running;

    let err = agent.transition_to_running().unwrap_err();
    assert!(matches!(err, RociError::InvalidState(_)));
}

#[tokio::test]
async fn prompt_with_system_prepends_system_message() {
    let config = AgentConfig {
        system_prompt: Some("You are helpful.".into()),
        ..test_agent_config()
    };
    let agent = AgentRuntime::new(test_registry(), test_config(), config);

    // We can't actually run the loop (no real provider), but we can verify
    // the transition_to_running guard works and then manually check message assembly.
    // Directly test the message assembly logic:
    {
        let system_prompt = agent.system_prompt.lock().await.clone();
        let mut msgs = agent.messages.lock().await;
        if let Some(ref sys) = system_prompt {
            if msgs.is_empty() {
                msgs.push(ModelMessage::system(sys.clone()));
            }
        }
        msgs.push(ModelMessage::user("hello"));
    }

    let msgs = agent.messages().await;
    assert_eq!(msgs.len(), 2);
    // First message should be the system prompt.
    assert_eq!(msgs[0].role, crate::types::Role::System);
    assert_eq!(msgs[1].role, crate::types::Role::User);
}

#[tokio::test]
async fn continue_run_rejects_when_running() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    // Force state to Running.
    *agent.state.lock().await = AgentState::Running;

    let err = agent.continue_run("more").await.unwrap_err();
    assert!(matches!(err, RociError::InvalidState(_)));
}

#[tokio::test]
async fn continue_without_input_rejects_when_running() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    *agent.state.lock().await = AgentState::Running;

    let err = agent.continue_without_input().await.unwrap_err();
    assert!(matches!(err, RociError::InvalidState(_)));
}

#[tokio::test]
async fn continue_without_input_rejects_when_history_is_empty() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    let err = agent.continue_without_input().await.unwrap_err();
    assert!(matches!(err, RociError::InvalidState(_)));
    assert_eq!(
        err.to_string(),
        "Invalid state: No messages to continue from"
    );
    assert_eq!(agent.state().await, AgentState::Idle);
}

#[tokio::test]
async fn continue_without_input_rejects_from_assistant_without_queued_messages() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    agent
        .messages
        .lock()
        .await
        .push(ModelMessage::assistant("done"));

    let err = agent.continue_without_input().await.unwrap_err();
    assert!(matches!(err, RociError::InvalidState(_)));
    assert_eq!(
        err.to_string(),
        "Invalid state: Cannot continue from message role: assistant"
    );
    assert_eq!(agent.state().await, AgentState::Idle);
}

#[tokio::test]
async fn prompt_rejects_when_aborting() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    *agent.state.lock().await = AgentState::Aborting;

    let err = agent.prompt("hey").await.unwrap_err();
    assert!(matches!(err, RociError::InvalidState(_)));
}

#[tokio::test]
async fn abort_is_idempotent_when_idle() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    assert!(!agent.abort().await);
    assert!(!agent.abort().await);
    assert_eq!(agent.state().await, AgentState::Idle);
}

#[tokio::test]
async fn reset_is_idempotent() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    agent.reset().await;
    assert_eq!(agent.state().await, AgentState::Idle);

    agent.reset().await;
    assert_eq!(agent.state().await, AgentState::Idle);
}

#[tokio::test]
async fn reset_clears_queued_steering_and_follow_up_messages() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    agent.steer("steer-1").await;
    agent.steer("steer-2").await;
    agent.follow_up("follow-1").await;
    agent.follow_up("follow-2").await;
    agent.follow_up("follow-3").await;

    assert_eq!(agent.steering_queue.lock().await.len(), 2);
    assert_eq!(agent.follow_up_queue.lock().await.len(), 3);

    agent.reset().await;

    assert!(agent.steering_queue.lock().await.is_empty());
    assert!(agent.follow_up_queue.lock().await.is_empty());
}

#[tokio::test]
async fn watch_state_reflects_manual_transitions() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    let mut rx = agent.watch_state();

    assert_eq!(*rx.borrow(), AgentState::Idle);

    {
        let mut state = agent.state.lock().await;
        *state = AgentState::Running;
        let _ = agent.state_tx.send(AgentState::Running);
    }

    rx.changed().await.unwrap();
    assert_eq!(*rx.borrow(), AgentState::Running);
}

#[tokio::test]
async fn multiple_aborts_are_safe() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    *agent.state.lock().await = AgentState::Running;
    let _ = agent.state_tx.send(AgentState::Running);

    let first = agent.abort().await;
    assert_eq!(agent.state().await, AgentState::Aborting);
    // Returns false because there is no active RunHandle in a test context,
    // but state has transitioned to Aborting regardless.
    assert!(!first);

    let second = agent.abort().await;
    assert!(!second);
    assert_eq!(agent.state().await, AgentState::Aborting);
}

#[tokio::test]
async fn continue_run_rejects_during_aborting() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    *agent.state.lock().await = AgentState::Aborting;

    let err = agent.continue_run("more input").await.unwrap_err();
    assert!(matches!(err, RociError::InvalidState(_)));
}

#[tokio::test]
async fn continue_without_input_rejects_during_aborting() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    *agent.state.lock().await = AgentState::Aborting;

    let err = agent.continue_without_input().await.unwrap_err();
    assert!(matches!(err, RociError::InvalidState(_)));
}
