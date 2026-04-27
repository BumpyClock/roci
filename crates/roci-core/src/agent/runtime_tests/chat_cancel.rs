use super::chat::{
    AgentRuntimeError, AgentRuntimeEvent, AgentRuntimeEventPayload, RuntimeSubscription, TurnStatus,
};
use super::support::*;
use super::*;
use crate::agent_loop::RunStatus;
use crate::models::ModelCapabilities;
use crate::provider::{ModelProvider, ProviderFactory, ProviderResponse};
use crate::types::{StreamEventType, TextStreamDelta, Usage};
use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use futures::StreamExt;
use std::sync::Arc;
use tokio::time::{timeout, Duration};

const RECV_TIMEOUT: Duration = Duration::from_secs(2);

fn runtime_with_chat_provider() -> AgentRuntime {
    let registry = registry_with_streaming_provider("stub", 8, 3);
    let mut config = test_agent_config();
    config.model = "stub:chat-cancel".parse().expect("stub model should parse");
    AgentRuntime::new(registry, test_config(), config)
}

fn runtime_with_blocking_provider() -> Arc<AgentRuntime> {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(BlockingTextFactory));

    let mut config = test_agent_config();
    config.model = "stub:chat-cancel-blocking"
        .parse()
        .expect("stub model should parse");

    Arc::new(AgentRuntime::new(Arc::new(registry), test_config(), config))
}

async fn recv_event(sub: &mut RuntimeSubscription) -> AgentRuntimeEvent {
    timeout(RECV_TIMEOUT, sub.recv())
        .await
        .expect("subscription should emit before timeout")
        .expect("subscription receive should succeed")
}

async fn recv_turn_started(sub: &mut RuntimeSubscription) -> TurnId {
    for _ in 0..8 {
        let event = recv_event(sub).await;
        if let AgentRuntimeEventPayload::TurnStarted { turn } = event.payload {
            return turn.turn_id;
        }
    }
    panic!("subscription did not emit TurnStarted");
}

async fn recv_turn_canceled(sub: &mut RuntimeSubscription) -> TurnId {
    for _ in 0..8 {
        let event = recv_event(sub).await;
        if let AgentRuntimeEventPayload::TurnCanceled { turn } = event.payload {
            assert_eq!(turn.status, TurnStatus::Canceled);
            return turn.turn_id;
        }
    }
    panic!("subscription did not emit TurnCanceled");
}

async fn wait_for_prompt_result(
    task: tokio::task::JoinHandle<Result<crate::agent_loop::RunResult, RociError>>,
) -> crate::agent_loop::RunResult {
    timeout(RECV_TIMEOUT, task)
        .await
        .expect("prompt task should finish before timeout")
        .expect("prompt task should not panic")
        .expect("prompt should return a run result")
}

fn assert_thread_has_canceled_turn(agent_thread: &ThreadSnapshot, turn_id: TurnId) {
    assert_eq!(agent_thread.active_turn_id, None);
    let turn = agent_thread
        .turns
        .iter()
        .find(|turn| turn.turn_id == turn_id)
        .expect("canceled turn should remain in thread snapshot");
    assert_eq!(turn.status, TurnStatus::Canceled);
    assert!(turn.completed_at.is_some());
}

#[tokio::test]
async fn cancel_completed_turn_returns_already_terminal() {
    let agent = runtime_with_chat_provider();

    let result = agent.prompt("complete").await.expect("prompt should run");
    assert_eq!(
        result.status,
        RunStatus::Completed,
        "error: {:?}",
        result.error
    );

    let thread = agent.read_snapshot().await.threads[0].clone();
    let turn_id = thread.turns[0].turn_id;
    let result = agent.cancel_turn(turn_id).await;

    match result {
        Err(AgentRuntimeError::AlreadyTerminal {
            turn_id: id,
            status,
        }) => {
            assert_eq!(id, turn_id);
            assert_eq!(status, TurnStatus::Completed);
        }
        Err(other) => panic!("expected AlreadyTerminal Completed, got {other:?}"),
        Ok(_) => panic!("canceling completed turn should fail"),
    }
}

#[tokio::test]
async fn cancel_turn_from_replaced_history_returns_stale_runtime() {
    let agent = runtime_with_chat_provider();

    let result = agent.prompt("old").await.expect("prompt should run");
    assert_eq!(
        result.status,
        RunStatus::Completed,
        "error: {:?}",
        result.error
    );
    let thread = agent.read_snapshot().await.threads[0].clone();
    let old_turn_id = thread.turns[0].turn_id;

    agent
        .replace_messages(vec![ModelMessage::user("replacement")])
        .await
        .expect("history should replace");

    let result = agent.cancel_turn(old_turn_id).await;
    match result {
        Err(AgentRuntimeError::StaleRuntime { thread_id, .. }) => {
            assert_eq!(thread_id, old_turn_id.thread_id());
        }
        Err(other) => panic!("expected StaleRuntime after replace_messages, got {other:?}"),
        Ok(_) => panic!("canceling stale turn should fail"),
    }
}

#[tokio::test]
async fn cancel_turn_from_reset_history_returns_stale_runtime() {
    let agent = runtime_with_chat_provider();

    let result = agent.prompt("old").await.expect("prompt should run");
    assert_eq!(
        result.status,
        RunStatus::Completed,
        "error: {:?}",
        result.error
    );
    let thread = agent.read_snapshot().await.threads[0].clone();
    let old_turn_id = thread.turns[0].turn_id;

    agent.reset().await;

    let result = agent.cancel_turn(old_turn_id).await;
    match result {
        Err(AgentRuntimeError::StaleRuntime { thread_id, .. }) => {
            assert_eq!(thread_id, old_turn_id.thread_id());
        }
        Err(other) => panic!("expected StaleRuntime after reset, got {other:?}"),
        Ok(_) => panic!("canceling stale turn should fail"),
    }
}

#[tokio::test]
async fn cancel_running_turn_emits_event_and_marks_thread_canceled() {
    let agent = runtime_with_blocking_provider();
    let mut sub = agent.subscribe(None).await;

    let prompt_agent = Arc::clone(&agent);
    let prompt_task = tokio::spawn(async move { prompt_agent.prompt("block").await });
    let turn_id = recv_turn_started(&mut sub).await;

    let result = agent.cancel_turn(turn_id).await;
    if let Err(err) = result {
        panic!("canceling running turn should succeed, got {err:?}");
    }

    let canceled_turn_id = recv_turn_canceled(&mut sub).await;
    assert_eq!(canceled_turn_id, turn_id);

    let result = wait_for_prompt_result(prompt_task).await;
    assert_eq!(
        result.status,
        RunStatus::Canceled,
        "error: {:?}",
        result.error
    );

    let thread = agent.read_snapshot().await.threads[0].clone();
    assert_thread_has_canceled_turn(&thread, turn_id);
    assert!(
        timeout(Duration::from_millis(100), sub.recv())
            .await
            .is_err(),
        "TurnCanceled should be terminal for semantic event subscribers"
    );
}

#[tokio::test]
async fn abort_running_turn_emits_event_and_marks_thread_canceled() {
    let agent = runtime_with_blocking_provider();
    let mut sub = agent.subscribe(None).await;

    let prompt_agent = Arc::clone(&agent);
    let prompt_task = tokio::spawn(async move { prompt_agent.prompt("block").await });
    let turn_id = recv_turn_started(&mut sub).await;

    assert!(agent.abort().await, "abort should signal active turn");

    let canceled_turn_id = recv_turn_canceled(&mut sub).await;
    assert_eq!(canceled_turn_id, turn_id);

    let result = wait_for_prompt_result(prompt_task).await;
    assert_eq!(
        result.status,
        RunStatus::Canceled,
        "error: {:?}",
        result.error
    );

    let thread = agent.read_snapshot().await.threads[0].clone();
    assert_thread_has_canceled_turn(&thread, turn_id);
    assert!(
        timeout(Duration::from_millis(100), sub.recv())
            .await
            .is_err(),
        "TurnCanceled should be terminal for semantic event subscribers"
    );
}

struct BlockingTextFactory;

impl ProviderFactory for BlockingTextFactory {
    fn provider_keys(&self) -> &[&str] {
        &["stub"]
    }

    fn create(
        &self,
        _config: &RociConfig,
        provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        Ok(Box::new(BlockingTextProvider {
            provider_key: provider_key.to_string(),
            model_id: model_id.to_string(),
            capabilities: ModelCapabilities::default(),
        }))
    }
}

struct BlockingTextProvider {
    provider_key: String,
    model_id: String,
    capabilities: ModelCapabilities,
}

#[async_trait]
impl ModelProvider for BlockingTextProvider {
    fn provider_name(&self) -> &str {
        &self.provider_key
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        Err(RociError::UnsupportedOperation(
            "stream-only blocking test provider".to_string(),
        ))
    }

    async fn stream_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        let events = stream::once(async {
            Ok(TextStreamDelta {
                text: "partial".to_string(),
                event_type: StreamEventType::TextDelta,
                tool_call: None,
                finish_reason: None,
                usage: Some(Usage::default()),
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            })
        })
        .chain(stream::pending());
        Ok(Box::pin(events))
    }
}
