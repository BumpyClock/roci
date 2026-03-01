use super::support::*;
use super::*;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn before_agent_start_hook_can_replace_initial_messages() {
    let created_models = Arc::new(Mutex::new(Vec::new()));
    let registry = registry_with_summary_provider("stub", "summary", created_models);
    let mut config = test_agent_config();
    config.model = "stub:run-model".parse().expect("stub model should parse");
    config.before_agent_start = Some(Arc::new(|payload| {
        Box::pin(async move {
            assert!(!payload.cancellation_token.is_cancelled());
            Ok(BeforeAgentStartHookResult::ReplaceMessages {
                messages: vec![
                    ModelMessage::system("hooked-system"),
                    ModelMessage::user("hooked-user"),
                ],
            })
        })
    }));
    let agent = AgentRuntime::new(registry, test_config(), config);

    let result = agent
        .prompt("original-user")
        .await
        .expect("run should produce a failed RunResult, not runtime error");

    assert_eq!(result.status, RunStatus::Failed);
    assert!(
        result
            .messages
            .iter()
            .any(|message| message.text() == "hooked-system"),
        "before_agent_start should be able to replace system context"
    );
    assert!(
        result
            .messages
            .iter()
            .any(|message| message.text() == "hooked-user"),
        "before_agent_start should be able to replace user context"
    );
    assert!(
        result
            .messages
            .iter()
            .all(|message| message.text() != "original-user"),
        "original input should be replaced when hook returns replacement messages"
    );
}

#[tokio::test]
async fn before_agent_start_hook_cancel_returns_canceled_result_and_restores_idle() {
    let mut config = test_agent_config();
    config.before_agent_start = Some(Arc::new(|payload| {
        Box::pin(async move {
            assert!(!payload.cancellation_token.is_cancelled());
            Ok(BeforeAgentStartHookResult::Cancel {
                reason: Some("blocked".to_string()),
            })
        })
    }));
    let agent = AgentRuntime::new(test_registry(), test_config(), config);

    let result = agent
        .prompt("hello")
        .await
        .expect("hook cancel should return canceled run result");
    assert_eq!(result.status, RunStatus::Canceled);
    assert_eq!(agent.state().await, AgentState::Idle);
    agent.wait_for_idle().await;
}

#[tokio::test]
async fn before_agent_start_hook_error_restores_idle_and_returns_runtime_error() {
    let mut config = test_agent_config();
    config.before_agent_start = Some(Arc::new(|_payload| {
        Box::pin(async { Err(RociError::InvalidArgument("boom".to_string())) })
    }));
    let agent = AgentRuntime::new(test_registry(), test_config(), config);

    let err = agent
        .prompt("hello")
        .await
        .expect_err("hook error should fail prompt");
    assert!(
        err.to_string().contains("before_agent_start hook failed"),
        "expected before_agent_start hook error prefix, got: {err}"
    );
    assert_eq!(agent.state().await, AgentState::Idle);
    agent.wait_for_idle().await;
}
