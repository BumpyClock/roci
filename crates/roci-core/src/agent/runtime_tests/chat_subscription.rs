use super::chat::{
    AgentRuntimeError, AgentRuntimeEvent, AgentRuntimeEventPayload, AgentRuntimeEventStore,
    ChatRuntimeConfig, InMemoryAgentRuntimeEventStore, RuntimeCursor, RuntimeSubscription,
    ThreadId,
};
use super::support::*;
use super::*;
use crate::agent_loop::RunStatus;
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{Mutex as TokioMutex, Notify};
use tokio::time::{timeout, Duration};

const RECV_TIMEOUT: Duration = Duration::from_secs(2);

fn runtime_with_chat_provider() -> AgentRuntime {
    runtime_with_chat_provider_config(ChatRuntimeConfig::default())
}

fn runtime_with_chat_provider_config(chat: ChatRuntimeConfig) -> AgentRuntime {
    let registry = registry_with_streaming_provider("stub", 8, 3);
    let mut config = test_agent_config();
    config.model = "stub:chat-subscription"
        .parse()
        .expect("stub model should parse");
    config.chat = chat;
    AgentRuntime::new(registry, test_config(), config)
}

async fn recv_event(sub: &mut RuntimeSubscription) -> AgentRuntimeEvent {
    timeout(RECV_TIMEOUT, sub.recv())
        .await
        .expect("subscription should emit before timeout")
        .expect("subscription receive should succeed")
}

fn payload_name(payload: &AgentRuntimeEventPayload) -> &'static str {
    match payload {
        AgentRuntimeEventPayload::TurnQueued { .. } => "turn_queued",
        AgentRuntimeEventPayload::TurnStarted { .. } => "turn_started",
        AgentRuntimeEventPayload::MessageStarted { .. } => "message_started",
        AgentRuntimeEventPayload::MessageUpdated { .. } => "message_updated",
        AgentRuntimeEventPayload::MessageCompleted { .. } => "message_completed",
        AgentRuntimeEventPayload::ToolStarted { .. } => "tool_started",
        AgentRuntimeEventPayload::ToolUpdated { .. } => "tool_updated",
        AgentRuntimeEventPayload::ToolCompleted { .. } => "tool_completed",
        AgentRuntimeEventPayload::TurnCompleted { .. } => "turn_completed",
        AgentRuntimeEventPayload::TurnFailed { .. } => "turn_failed",
        AgentRuntimeEventPayload::TurnCanceled { .. } => "turn_canceled",
    }
}

fn payload_names(events: &[AgentRuntimeEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| payload_name(&event.payload))
        .collect()
}

fn payload_index(names: &[&str], target: &str) -> usize {
    names
        .iter()
        .position(|name| *name == target)
        .unwrap_or_else(|| panic!("missing {target} in {names:?}"))
}

fn assert_strictly_increasing_seq(events: &[AgentRuntimeEvent]) {
    for pair in events.windows(2) {
        assert!(
            pair[0].seq < pair[1].seq,
            "seq must strictly increase: {} then {}",
            pair[0].seq,
            pair[1].seq
        );
    }
}

fn assert_no_snapshot_updated(events: &[AgentRuntimeEvent]) {
    for event in events {
        let payload = serde_json::to_value(&event.payload).expect("payload serializes");
        assert_ne!(payload["type"], "snapshot_updated");
    }
}

#[derive(Default)]
struct DelayedAppendStore {
    inner: InMemoryAgentRuntimeEventStore,
    append_started: Notify,
    release_append: Notify,
    append_count: TokioMutex<usize>,
}

#[async_trait]
impl AgentRuntimeEventStore for DelayedAppendStore {
    async fn append(&self, event: AgentRuntimeEvent) -> Result<RuntimeCursor, AgentRuntimeError> {
        {
            let mut count = self.append_count.lock().await;
            *count += 1;
            if *count == 1 {
                self.append_started.notify_waiters();
                drop(count);
                self.release_append.notified().await;
            }
        }
        self.inner.append(event).await
    }

    async fn events_after(
        &self,
        cursor: RuntimeCursor,
    ) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
        self.inner.events_after(cursor).await
    }

    async fn invalidate_thread(
        &self,
        thread_id: ThreadId,
        latest_seq: u64,
    ) -> Result<(), AgentRuntimeError> {
        self.inner.invalidate_thread(thread_id, latest_seq).await
    }
}

#[tokio::test]
async fn subscribe_before_prompt_receives_live_semantic_events_in_order() {
    let agent = runtime_with_chat_provider();
    let mut sub = agent.subscribe(None).await;

    let result = agent.prompt("hello").await.expect("prompt should run");
    assert_eq!(
        result.status,
        RunStatus::Completed,
        "error: {:?}",
        result.error
    );

    let mut events = Vec::new();
    while !matches!(
        events
            .last()
            .map(|event: &AgentRuntimeEvent| &event.payload),
        Some(AgentRuntimeEventPayload::TurnCompleted { .. })
    ) {
        events.push(recv_event(&mut sub).await);
    }

    assert_strictly_increasing_seq(&events);
    assert_no_snapshot_updated(&events);

    let names = payload_names(&events);
    let turn_queued = payload_index(&names, "turn_queued");
    let turn_started = payload_index(&names, "turn_started");
    let message_started = payload_index(&names, "message_started");
    let message_completed = payload_index(&names, "message_completed");
    let turn_completed = payload_index(&names, "turn_completed");

    assert!(
        turn_queued < turn_started,
        "TurnQueued should precede TurnStarted: {names:?}"
    );
    assert!(
        turn_queued < message_started,
        "TurnQueued should precede message lifecycle events: {names:?}"
    );
    assert!(
        message_started < message_completed,
        "MessageStarted should precede MessageCompleted: {names:?}"
    );
    assert!(
        message_completed < turn_completed,
        "MessageCompleted should precede TurnCompleted: {names:?}"
    );
    assert!(
        names
            .iter()
            .enumerate()
            .any(|(index, name)| *name == "message_started" && index < turn_started),
        "queued input message should be emitted before turn starts: {names:?}"
    );
    assert!(
        names
            .iter()
            .enumerate()
            .any(|(index, name)| *name == "message_started" && index > turn_started),
        "assistant message should be emitted after turn starts: {names:?}"
    );
}

#[tokio::test]
async fn live_event_broadcast_waits_for_async_store_append() {
    let store = Arc::new(DelayedAppendStore::default());
    let agent = runtime_with_chat_provider_config(ChatRuntimeConfig {
        event_store: Some(store.clone()),
        ..ChatRuntimeConfig::default()
    });
    let mut sub = agent.subscribe(None).await;

    let prompt = tokio::spawn(async move { agent.prompt("hello").await });
    store.append_started.notified().await;

    let no_event_before_append = timeout(Duration::from_millis(50), sub.recv()).await;
    assert!(
        no_event_before_append.is_err(),
        "event broadcasted before async store append completed"
    );

    store.release_append.notify_waiters();
    let first_event = recv_event(&mut sub).await;
    assert!(matches!(
        first_event.payload,
        AgentRuntimeEventPayload::TurnQueued { .. }
    ));

    let result = prompt
        .await
        .expect("prompt task should not panic")
        .expect("prompt should run");
    assert_eq!(result.status, RunStatus::Completed);
}

#[tokio::test]
async fn subscribe_after_cursor_replays_tail_then_receives_fresh_live_events_without_duplicates() {
    let agent = runtime_with_chat_provider();

    let first = agent
        .prompt("first")
        .await
        .expect("first prompt should run");
    assert_eq!(
        first.status,
        RunStatus::Completed,
        "error: {:?}",
        first.error
    );

    let thread = agent.read_snapshot().await.threads[0].clone();
    let cursor = RuntimeCursor::new(thread.thread_id, 2);
    let mut sub = agent.subscribe(Some(cursor)).await;
    let replayed = sub.replay().expect("cursor should replay retained events");

    assert!(
        replayed.iter().all(|event| event.seq > cursor.seq),
        "replay must only return events after cursor"
    );
    assert_strictly_increasing_seq(&replayed);
    assert_no_snapshot_updated(&replayed);

    let mut seen = replayed
        .iter()
        .map(|event| event.seq)
        .collect::<HashSet<_>>();
    assert_eq!(
        seen.len(),
        replayed.len(),
        "replay should not duplicate seqs"
    );
    let last_replayed_seq = replayed
        .last()
        .expect("cursor should replay at least one event")
        .seq;

    let second = agent
        .prompt("second")
        .await
        .expect("second prompt should run");
    assert_eq!(
        second.status,
        RunStatus::Completed,
        "error: {:?}",
        second.error
    );

    let live = recv_event(&mut sub).await;
    assert!(
        live.seq > last_replayed_seq,
        "live event should be fresh after replay: replayed {last_replayed_seq}, live {}",
        live.seq
    );
    assert!(
        seen.insert(live.seq),
        "live event duplicated replayed seq {}",
        live.seq
    );
    assert_no_snapshot_updated(&[live]);
}

#[tokio::test]
async fn stale_cursor_returns_stale_runtime_from_subscription_replay() {
    let agent = runtime_with_chat_provider_config(ChatRuntimeConfig {
        replay_capacity: 2,
        ..ChatRuntimeConfig::default()
    });

    let thread_id = agent.read_snapshot().await.threads[0].thread_id;
    let result = agent
        .prompt("evict old events")
        .await
        .expect("prompt should run");
    assert_eq!(
        result.status,
        RunStatus::Completed,
        "error: {:?}",
        result.error
    );

    let mut sub = agent
        .subscribe(Some(RuntimeCursor::new(thread_id, 0)))
        .await;
    let replay_err = sub.replay().expect_err("evicted cursor should be stale");

    assert!(
        matches!(replay_err, AgentRuntimeError::StaleRuntime { .. }),
        "expected stale runtime from replay, got {replay_err:?}"
    );

    let recv_err = timeout(RECV_TIMEOUT, sub.recv())
        .await
        .expect("stale subscription should fail before timeout")
        .expect_err("stale subscription should fail before live events");
    assert!(
        matches!(recv_err, AgentRuntimeError::StaleRuntime { .. }),
        "expected stale runtime from recv, got {recv_err:?}"
    );
}

#[tokio::test]
async fn rewrite_invalidates_prior_subscription_cursor() {
    let agent = runtime_with_chat_provider();

    let result = agent.prompt("old").await.expect("prompt should run");
    assert_eq!(
        result.status,
        RunStatus::Completed,
        "error: {:?}",
        result.error
    );
    let thread = agent.read_snapshot().await.threads[0].clone();
    let old_cursor = RuntimeCursor::new(thread.thread_id, thread.last_seq);

    agent
        .replace_messages(vec![ModelMessage::user("replacement")])
        .await
        .expect("history should replace");

    let sub = agent.subscribe(Some(old_cursor)).await;
    let replay_err = sub.replay().expect_err("rewrite should stale old cursor");
    assert!(
        matches!(replay_err, AgentRuntimeError::StaleRuntime { .. }),
        "expected stale runtime after rewrite, got {replay_err:?}"
    );
}

#[tokio::test]
async fn reset_invalidates_prior_subscription_cursor() {
    let agent = runtime_with_chat_provider();

    let result = agent.prompt("old").await.expect("prompt should run");
    assert_eq!(
        result.status,
        RunStatus::Completed,
        "error: {:?}",
        result.error
    );
    let thread = agent.read_snapshot().await.threads[0].clone();
    let old_cursor = RuntimeCursor::new(thread.thread_id, thread.last_seq);

    agent.reset().await;

    let sub = agent.subscribe(Some(old_cursor)).await;
    let replay_err = sub.replay().expect_err("reset should stale old cursor");
    assert!(
        matches!(replay_err, AgentRuntimeError::StaleRuntime { .. }),
        "expected stale runtime after reset, got {replay_err:?}"
    );
}

#[tokio::test]
async fn event_stream_has_no_snapshot_updated_payload() {
    let agent = runtime_with_chat_provider();
    let mut sub = agent.subscribe(None).await;

    let result = agent.prompt("hello").await.expect("prompt should run");
    assert_eq!(
        result.status,
        RunStatus::Completed,
        "error: {:?}",
        result.error
    );

    let mut events = Vec::new();
    while !matches!(
        events
            .last()
            .map(|event: &AgentRuntimeEvent| &event.payload),
        Some(AgentRuntimeEventPayload::TurnCompleted { .. })
    ) {
        events.push(recv_event(&mut sub).await);
    }

    assert_no_snapshot_updated(&events);
}
