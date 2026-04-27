# AgentRuntime Chat Projection Semantics Implementation Plan

> **For agentic workers:** Execute task-by-task. Use subagent-driven development when available, otherwise run tasks inline with review checkpoints. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `roci-core` own chat runtime semantics in-memory, with `read_snapshot()` as reconnect truth and `subscribe(cursor)` as bounded semantic replay plus live stream.

**Architecture:** Add a new `agent::runtime::chat` projection layer inside `roci-core`. `agent_loop` stays a low-level producer of provider/tool lifecycle events; `AgentRuntime` creates semantic turns, projects low-level message/tool events into thread-aware snapshots, and owns cancellation/reconnect behavior. Default durability is in-memory snapshot state plus a bounded per-thread semantic event ring; an optional `RuntimeEventStore` extends replay depth but never becomes the state authority.

**Tech Stack:** Rust 2021, Tokio (`Mutex`, `watch`, `broadcast`), `chrono`, `uuid`, `serde`, `thiserror`, existing `ModelMessage` / `AgentEvent` / `RunResult` types.

---

## Assumptions

- V1 keeps **one stable default thread per `AgentRuntime`**. Snapshot/event types are thread-aware now; multi-thread creation/switch APIs stay out of this scope.
- Existing `prompt()`, `continue_run()`, `continue_without_input()`, `steer()`, `follow_up()`, `reset()`, `compact()`, and `summarize_branch_entries()` stay available unless explicitly called out below.
- `AgentConfig.event_sink` stays as the low-level `AgentEvent` sink for CLI/examples/tests. New host integrations should prefer `read_snapshot()` + `subscribe()`.
- `AgentEvent::TurnStart` / `TurnEnd` remain loop-internal iteration markers. **Semantic chat turns** are created and terminated by `AgentRuntime`, not by re-exporting those loop events.
- Manual history rewrites (`replace_messages`, `compact`, `reset`) are out-of-band relative to incremental replay. V1 resolves that by invalidating subscriptions with `StaleRuntime`, not by inventing a synthetic `SnapshotUpdated` event.

## File Structure

- Modify: `crates/roci-core/src/agent/runtime.rs:17-41,82-156`
  Add `chat` module wiring, re-exports, runtime field(s), and default-thread initialization.
- Modify: `crates/roci-core/src/agent/mod.rs:13-20`
  Re-export new chat runtime public types.
- Modify: `crates/roci-core/src/agent/runtime/config.rs:29-105`
  Add `ChatRuntimeConfig` to `AgentConfig` and update constructors/call sites.
- Modify: `crates/roci-core/src/agent/runtime/events.rs:1-39`
  Replace shallow `turn_index` interception with semantic projection sink wiring.
- Modify: `crates/roci-core/src/agent/runtime/lifecycle.rs:7-168`
  Create semantic turns, add `cancel_turn`, keep `abort()` as compatibility sugar.
- Modify: `crates/roci-core/src/agent/runtime/run_loop.rs:42-273`
  Register queued/running/terminal turn semantics, use projected state as runtime truth, and stop relying on `result.messages` as the first state commit.
- Modify: `crates/roci-core/src/agent/runtime/state.rs:7-107`
  Implement `read_snapshot`, `read_thread`, and derive legacy shallow state/snapshot accessors from chat state.
- Modify: `crates/roci-core/src/agent/runtime/mutations.rs:10-138`
  Route `replace_messages` and queue clearing through chat-aware invalidation rules.
- Modify: `crates/roci-core/src/agent/runtime/summary.rs:29-66`
  Make manual compaction update chat truth and invalidate incremental subscribers.
- Modify: `crates/roci-core/src/agent/subagents/launcher.rs:64-141`
  Seed child runtime history through a chat-aware bootstrap helper instead of public `replace_messages()`.
- Modify: `crates/roci-core/src/agent/runtime_tests/mod.rs:3-17`
  Register new chat runtime test modules.
- Modify: `crates/roci-core/src/agent/runtime_tests/support.rs:112-145`
  Add `AgentConfig.chat` defaults plus streaming/cancel test helpers.
- Modify: `crates/roci-cli/src/chat.rs:90-128`
  Compile-fix `AgentConfig` initializer; keep raw `AgentEvent` UI path unchanged for now.
- Modify: `examples/agent_runtime.rs:1-190`
  Show `read_snapshot`, `read_thread`, `subscribe`, `cancel_turn`, and note `abort()` compatibility.
- Modify: `docs/ARCHITECTURE.md:114-129`
  Document new `agent_loop` vs `runtime::chat` layering and replay semantics.

- Create: `crates/roci-core/src/agent/runtime/chat/mod.rs`
  Public module root; re-export domain/event/projector/subscription/store/error.
- Create: `crates/roci-core/src/agent/runtime/chat/domain.rs`
  `RuntimeSnapshot`, `ThreadSnapshot`, `TurnSnapshot`, `MessageSnapshot`, `ToolExecutionSnapshot`, IDs, and status enums.
- Create: `crates/roci-core/src/agent/runtime/chat/event.rs`
  `RuntimeCursor`, `AgentRuntimeEvent` envelope, semantic payload enum.
- Create: `crates/roci-core/src/agent/runtime/chat/projector.rs`
  Authoritative in-memory chat state, event projection, ID allocation, seq allocation, and invalidation logic.
- Create: `crates/roci-core/src/agent/runtime/chat/subscription.rs`
  `RuntimeSubscription`, replay/live mux, stale-on-gap behavior.
- Create: `crates/roci-core/src/agent/runtime/chat/store.rs`
  `RuntimeEventStore` trait and in-memory reference implementation.
- Create: `crates/roci-core/src/agent/runtime/chat/error.rs`
  `AgentRuntimeError` variants.

- Create: `crates/roci-core/src/agent/runtime_tests/chat_contracts.rs`
  Contract tests for IDs, statuses, config defaults, and exported type shapes.
- Create: `crates/roci-core/src/agent/runtime_tests/chat_projection.rs`
  Pure projection tests using synthetic `AgentEvent` inputs.
- Create: `crates/roci-core/src/agent/runtime_tests/chat_runtime_projection.rs`
  End-to-end `AgentRuntime` tests covering incremental snapshot updates during a live run.
- Create: `crates/roci-core/src/agent/runtime_tests/chat_subscription.rs`
  Replay/live/store/stale tests.
- Create: `crates/roci-core/src/agent/runtime_tests/chat_cancellation.rs`
  `cancel_turn`, `abort()` compatibility, and invalidation tests.

---

### Task 1: Define Chat Runtime Contracts and Config Seam

**Files:**
- Create: `crates/roci-core/src/agent/runtime/chat/mod.rs`
- Create: `crates/roci-core/src/agent/runtime/chat/domain.rs`
- Create: `crates/roci-core/src/agent/runtime/chat/event.rs`
- Create: `crates/roci-core/src/agent/runtime/chat/error.rs`
- Modify: `crates/roci-core/src/agent/runtime.rs:17-41,82-156`
- Modify: `crates/roci-core/src/agent/runtime/config.rs:29-105`
- Modify: `crates/roci-core/src/agent/mod.rs:13-20`
- Modify: `crates/roci-core/src/agent/runtime_tests/mod.rs:3-17`
- Test: `crates/roci-core/src/agent/runtime_tests/chat_contracts.rs`

**Acceptance criteria:**
- `roci_core::agent::runtime::chat` exports thread-aware snapshot/event/error types with no transport/persistence policy baked in.
- `TurnId` and `MessageId` encode thread revision so out-of-band rewrites can surface `StaleRuntime` instead of silent `NotFound`.
- `AgentConfig` gains a minimal `chat: ChatRuntimeConfig` field with `Default` = in-memory replay only (`replay_capacity = 512`, `event_store = None`).

**Tracking:**
- Tasque child: `tsq-c9dvskta.1`
- Dependencies: none

- [ ] **Step 1: Write the failing contract test**

```rust
use super::support::*;
use crate::agent::runtime::chat::{
    AgentRuntimeError, ChatRuntimeConfig, MessageId, MessageStatus, RuntimeCursor,
    RuntimeEventPayload, ThreadId, TurnId, TurnStatus,
};

#[test]
fn chat_runtime_config_defaults_to_bounded_in_memory_replay() {
    let config = ChatRuntimeConfig::default();
    assert_eq!(config.replay_capacity, 512);
    assert!(config.event_store.is_none());
}

#[test]
fn turn_and_message_ids_carry_thread_revision() {
    let thread_id = ThreadId::new();
    let turn_id = TurnId::new(thread_id, 7, 3);
    let message_id = MessageId::new(thread_id, 7, 9);

    assert_eq!(turn_id.thread_id(), thread_id);
    assert_eq!(turn_id.revision(), 7);
    assert_eq!(message_id.thread_id(), thread_id);
    assert_eq!(message_id.revision(), 7);
}

#[test]
fn stale_runtime_error_reports_requested_and_oldest_seq() {
    let thread_id = ThreadId::new();
    let err = AgentRuntimeError::StaleRuntime {
        thread_id,
        requested_seq: 4,
        oldest_available_seq: 12,
        latest_seq: 19,
    };
    assert!(err.to_string().contains("requested seq 4"));
    assert!(err.to_string().contains("oldest available 12"));
}

#[test]
fn semantic_payload_set_matches_target_contract() {
    let payload_names = [
        RuntimeEventPayload::turn_queued_name(),
        RuntimeEventPayload::turn_started_name(),
        RuntimeEventPayload::message_started_name(),
        RuntimeEventPayload::message_updated_name(),
        RuntimeEventPayload::message_completed_name(),
        RuntimeEventPayload::tool_started_name(),
        RuntimeEventPayload::tool_updated_name(),
        RuntimeEventPayload::tool_completed_name(),
        RuntimeEventPayload::turn_completed_name(),
        RuntimeEventPayload::turn_failed_name(),
        RuntimeEventPayload::turn_canceled_name(),
    ];
    assert_eq!(payload_names.len(), 11);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::chat_contracts::"`
Expected: FAIL with unresolved imports / missing `chat` module / missing `AgentConfig.chat` field.

- [ ] **Step 3: Implement minimal code**

```rust
// crates/roci-core/src/agent/runtime/chat/domain.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThreadId(Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TurnId {
    thread_id: ThreadId,
    revision: u64,
    ordinal: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId {
    thread_id: ThreadId,
    revision: u64,
    ordinal: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Streaming,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageSnapshot {
    pub message_id: MessageId,
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub status: MessageStatus,
    pub payload: ModelMessage,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolExecutionSnapshot {
    pub tool_call_id: String,
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub tool_name: String,
    pub args: serde_json::Value,
    pub status: ToolStatus,
    pub partial_result: Option<ToolUpdatePayload>,
    pub final_result: Option<AgentToolResult>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnSnapshot {
    pub turn_id: TurnId,
    pub status: TurnStatus,
    pub message_ids: Vec<MessageId>,
    pub active_tool_call_ids: Vec<String>,
    pub error: Option<String>,
    pub queued_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadSnapshot {
    pub thread_id: ThreadId,
    pub revision: u64,
    pub last_seq: u64,
    pub active_turn_id: Option<TurnId>,
    pub turns: Vec<TurnSnapshot>,
    pub messages: Vec<MessageSnapshot>,
    pub tools: Vec<ToolExecutionSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub schema_version: u16,
    pub threads: Vec<ThreadSnapshot>,
}

// crates/roci-core/src/agent/runtime/chat/event.rs
pub const CHAT_RUNTIME_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCursor {
    pub thread_id: ThreadId,
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRuntimeEvent {
    pub schema_version: u16,
    pub seq: u64,
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub timestamp: DateTime<Utc>,
    pub payload: RuntimeEventPayload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEventPayload {
    TurnQueued { turn: TurnSnapshot },
    TurnStarted { turn: TurnSnapshot },
    MessageStarted { message: MessageSnapshot },
    MessageUpdated { message: MessageSnapshot },
    MessageCompleted { message: MessageSnapshot },
    ToolStarted { tool: ToolExecutionSnapshot },
    ToolUpdated { tool: ToolExecutionSnapshot },
    ToolCompleted { tool: ToolExecutionSnapshot },
    TurnCompleted { turn: TurnSnapshot },
    TurnFailed { turn: TurnSnapshot, error: String },
    TurnCanceled { turn: TurnSnapshot },
}

// crates/roci-core/src/agent/runtime/chat/error.rs
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum AgentRuntimeError {
    #[error("thread not found: {thread_id}")]
    ThreadNotFound { thread_id: ThreadId },
    #[error("turn not found: {turn_id}")]
    TurnNotFound { turn_id: TurnId },
    #[error("turn already terminal: {turn_id} ({status:?})")]
    AlreadyTerminal { turn_id: TurnId, status: TurnStatus },
    #[error(
        "runtime cursor is stale for thread {thread_id}: requested seq {requested_seq}, oldest available {oldest_available_seq}, latest seq {latest_seq}"
    )]
    StaleRuntime {
        thread_id: ThreadId,
        requested_seq: u64,
        oldest_available_seq: u64,
        latest_seq: u64,
    },
    #[error("runtime event store failed: {message}")]
    EventStore { message: String },
    #[error("runtime invariant violated: {message}")]
    Invariant { message: String },
}

// crates/roci-core/src/agent/runtime/config.rs
pub struct ChatRuntimeConfig {
    pub replay_capacity: usize,
    pub event_store: Option<Arc<dyn RuntimeEventStore>>,
}

impl Default for ChatRuntimeConfig {
    fn default() -> Self {
        Self {
            replay_capacity: 512,
            event_store: None,
        }
    }
}

pub struct AgentConfig {
    // existing fields ...
    pub chat: ChatRuntimeConfig,
}
```

- [ ] **Step 4: Run focused verification**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::chat_contracts::"`
Expected: PASS

- [ ] **Step 5: Run broader relevant checks**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::"`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/roci-core/src/agent/runtime.rs \
  crates/roci-core/src/agent/mod.rs \
  crates/roci-core/src/agent/runtime/config.rs \
  crates/roci-core/src/agent/runtime/chat/mod.rs \
  crates/roci-core/src/agent/runtime/chat/domain.rs \
  crates/roci-core/src/agent/runtime/chat/event.rs \
  crates/roci-core/src/agent/runtime/chat/error.rs \
  crates/roci-core/src/agent/runtime_tests/mod.rs \
  crates/roci-core/src/agent/runtime_tests/chat_contracts.rs

git commit -m "feat: define chat runtime contracts"
```

### Task 2: Implement In-Memory Chat Projection State and Default Thread Model

**Files:**
- Create: `crates/roci-core/src/agent/runtime/chat/projector.rs`
- Modify: `crates/roci-core/src/agent/runtime.rs:82-156`
- Modify: `crates/roci-core/src/agent/runtime/state.rs:7-107`
- Test: `crates/roci-core/src/agent/runtime_tests/chat_projection.rs`

**Acceptance criteria:**
- `AgentRuntime::new()` creates one stable default thread and stores all chat state there.
- A new projector owns the authoritative `RuntimeSnapshot`, per-thread seq allocation, message/tool ID allocation, and turn/message/tool mutation helpers.
- `read_snapshot()` / `read_thread()` can return meaningful data without running a real provider.

**Tracking:**
- Tasque child: `tsq-c9dvskta.2`
- Dependencies: `Task 1 blocks this`

- [ ] **Step 1: Write the failing projection tests**

```rust
use super::support::*;
use crate::agent::runtime::chat::{MessageStatus, TurnStatus};
use crate::agent::runtime::AgentRuntime;
use crate::agent_loop::AgentEvent;
use crate::types::{ModelMessage, StreamEventType, TextStreamDelta};

#[tokio::test]
async fn read_snapshot_starts_with_one_default_thread() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    let snapshot = agent.read_snapshot().await;
    assert_eq!(snapshot.threads.len(), 1);
    assert_eq!(snapshot.threads[0].turns.len(), 0);
    assert_eq!(snapshot.threads[0].messages.len(), 0);
}

#[tokio::test]
async fn projector_tracks_partial_assistant_message_and_thread_scoped_seq() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    let thread_id = agent.read_snapshot().await.threads[0].thread_id;
    let turn_id = agent.chat_queue_turn_for_test(vec![ModelMessage::user("hello")]).await;
    agent.chat_mark_turn_started_for_test(turn_id).await;
    agent.chat_apply_agent_event_for_test(
        turn_id,
        AgentEvent::MessageStart { message: ModelMessage::assistant("") },
    ).await;
    agent.chat_apply_agent_event_for_test(
        turn_id,
        AgentEvent::MessageUpdate {
            message: ModelMessage::assistant("par"),
            assistant_message_event: TextStreamDelta {
                event_type: StreamEventType::TextDelta,
                text: "par".into(),
                reasoning: None,
                tool_call: None,
            },
        },
    ).await;

    let thread = agent.read_thread(thread_id).await.unwrap();
    assert_eq!(thread.last_seq, 4);
    assert_eq!(thread.turns[0].status, TurnStatus::Running);
    assert_eq!(thread.messages.last().unwrap().status, MessageStatus::Streaming);
    assert_eq!(thread.messages.last().unwrap().payload.text(), "par");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::chat_projection::"`
Expected: FAIL with missing projector helpers / missing `read_snapshot` / missing `read_thread`.

- [ ] **Step 3: Implement minimal code**

```rust
// crates/roci-core/src/agent/runtime/chat/projector.rs
pub(super) struct ChatRuntime {
    state: Mutex<ChatRuntimeState>,
}

struct ChatRuntimeState {
    snapshot: RuntimeSnapshot,
    default_thread_id: ThreadId,
    next_revision: u64,
    next_turn_ordinal: u64,
    next_message_ordinal: u64,
    next_seq_by_thread: HashMap<ThreadId, u64>,
    open_message_by_turn: HashMap<TurnId, MessageId>,
}

impl ChatRuntime {
    pub(super) fn new(config: &ChatRuntimeConfig) -> Self;
    pub(super) async fn read_snapshot(&self) -> RuntimeSnapshot;
    pub(super) async fn read_thread(
        &self,
        thread_id: ThreadId,
    ) -> Result<ThreadSnapshot, AgentRuntimeError>;
    pub(super) async fn queue_turn(
        &self,
        seed_messages: Vec<ModelMessage>,
    ) -> (ThreadId, TurnId);
    pub(super) async fn mark_turn_started(&self, turn_id: TurnId) -> TurnSnapshot;
    pub(super) async fn apply_agent_event(
        &self,
        turn_id: TurnId,
        event: &AgentEvent,
    ) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError>;
}

// crates/roci-core/src/agent/runtime/state.rs
impl AgentRuntime {
    pub async fn read_snapshot(&self) -> RuntimeSnapshot {
        self.chat.read_snapshot().await
    }

    pub async fn read_thread(
        &self,
        thread_id: ThreadId,
    ) -> Result<ThreadSnapshot, AgentRuntimeError> {
        self.chat.read_thread(thread_id).await
    }

    pub async fn snapshot(&self) -> AgentSnapshot {
        let thread = self.chat.default_thread().await;
        AgentSnapshot {
            state: Self::legacy_state_from_thread(&thread),
            turn_index: thread.turns.len(),
            message_count: thread.messages.len(),
            is_streaming: thread.active_turn_id.is_some(),
            last_error: thread.turns.iter().rev().find_map(|turn| turn.error.clone()),
        }
    }
}
```

- [ ] **Step 4: Run focused verification**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::chat_projection::"`
Expected: PASS

- [ ] **Step 5: Run broader relevant checks**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::snapshot"`
Expected: PASS with legacy shallow snapshot tests deriving from chat state.

- [ ] **Step 6: Commit**

```bash
git add crates/roci-core/src/agent/runtime.rs \
  crates/roci-core/src/agent/runtime/state.rs \
  crates/roci-core/src/agent/runtime/chat/projector.rs \
  crates/roci-core/src/agent/runtime_tests/chat_projection.rs

git commit -m "feat: add in-memory chat projection state"
```

### Task 3: Wire AgentRuntime Lifecycle and AgentEvent Projection into Chat State

**Files:**
- Modify: `crates/roci-core/src/agent/runtime/events.rs:1-39`
- Modify: `crates/roci-core/src/agent/runtime/run_loop.rs:42-273`
- Modify: `crates/roci-core/src/agent/runtime/lifecycle.rs:7-168`
- Modify: `crates/roci-core/src/agent/runtime/state.rs:7-107`
- Modify: `crates/roci-core/src/agent/runtime.rs:82-156`
- Modify: `crates/roci-core/src/agent/runtime_tests/support.rs:112-145`
- Test: `crates/roci-core/src/agent/runtime_tests/chat_runtime_projection.rs`

**Acceptance criteria:**
- `prompt()`, `continue_run()`, and `continue_without_input()` create semantic queued turns before any provider work.
- `TurnQueued` and `TurnStarted` are emitted by runtime lifecycle; low-level `AgentEvent::Message*` and `ToolExecution*` are projected into the currently running semantic turn.
- `read_snapshot()` reflects assistant/tool progress during the run, not only after `RunResult` returns.

**Tracking:**
- Tasque child: `tsq-c9dvskta.3`
- Dependencies: `Task 2 blocks this`

- [ ] **Step 1: Write the failing end-to-end runtime projection test**

```rust
use super::support::*;
use crate::agent::runtime::chat::{MessageStatus, TurnStatus};
use std::sync::Arc;
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn read_thread_updates_before_prompt_future_resolves() {
    let registry = registry_with_streaming_provider(vec![
        StreamingStep::TextDelta("partial"),
        StreamingStep::Delay(Duration::from_millis(150)),
        StreamingStep::Done,
    ]);
    let agent = Arc::new(AgentRuntime::new(registry, test_config(), test_agent_config()));
    let thread_id = agent.read_snapshot().await.threads[0].thread_id;

    let running = {
        let agent = agent.clone();
        tokio::spawn(async move { agent.prompt("hello").await })
    };

    sleep(Duration::from_millis(50)).await;
    let thread = agent.read_thread(thread_id).await.unwrap();
    assert_eq!(thread.turns.last().unwrap().status, TurnStatus::Running);
    assert_eq!(thread.messages.last().unwrap().status, MessageStatus::Streaming);
    assert_eq!(thread.messages.last().unwrap().payload.text(), "partial");

    let result = running.await.expect("join").expect("prompt should finish");
    assert_eq!(result.messages.last().unwrap().text(), "partial");
    let thread = agent.read_thread(thread_id).await.unwrap();
    assert_eq!(thread.turns.last().unwrap().status, TurnStatus::Completed);
    assert_eq!(thread.messages.last().unwrap().status, MessageStatus::Completed);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::chat_runtime_projection::" -- --nocapture`
Expected: FAIL because runtime state only updates after `run_loop()` completes.

- [ ] **Step 3: Implement minimal code**

```rust
// crates/roci-core/src/agent/runtime/lifecycle.rs
pub async fn prompt(&self, text: impl Into<String>) -> Result<RunResult, RociError> {
    self.transition_to_running()?;
    let seed_messages = self.build_prompt_seed_messages(text.into()).await?;
    let (_thread_id, turn_id) = self.chat.queue_turn(seed_messages.clone()).await;
    self.sync_legacy_watchers_from_chat().await;
    self.run_loop(turn_id, seed_messages).await
}

// crates/roci-core/src/agent/runtime/run_loop.rs
pub(super) async fn run_loop(
    &self,
    turn_id: TurnId,
    initial_messages: Vec<ModelMessage>,
) -> Result<RunResult, RociError> {
    self.chat.mark_turn_started(turn_id).await;
    self.sync_legacy_watchers_from_chat().await;

    let intercepting_sink = self.build_projecting_sink(turn_id);
    let mut request = RunRequest::new(model, initial_messages)
        .with_agent_event_sink(intercepting_sink);

    let run_result = /* existing runner invocation */;

    match &run_result {
        Ok(result) if self.chat.is_turn_canceled(turn_id).await => {
            self.chat.finish_turn_canceled(turn_id).await;
        }
        Ok(result) if result.status == RunStatus::Completed => {
            self.chat.finish_turn_completed(turn_id, result.messages.clone()).await;
        }
        Ok(result) if result.status == RunStatus::Failed => {
            self.chat.finish_turn_failed(turn_id, result.error.clone().unwrap_or_default()).await;
        }
        Ok(_) => {
            self.chat.finish_turn_canceled(turn_id).await;
        }
        Err(err) => {
            self.chat.finish_turn_failed(turn_id, err.to_string()).await;
        }
    }

    self.sync_legacy_watchers_from_chat().await;
    run_result
}

// crates/roci-core/src/agent/runtime/events.rs
pub(super) fn build_projecting_sink(&self, turn_id: TurnId) -> AgentEventSink {
    let chat = self.chat.clone();
    let original_sink = self.config.event_sink.clone();
    Arc::new(move |event: AgentEvent| {
        let chat = chat.clone();
        let event_for_host = event.clone();
        tokio::spawn(async move {
            let _ = chat.apply_agent_event(turn_id, &event).await;
        });
        if let Some(ref sink) = original_sink {
            sink(event_for_host);
        }
    })
}
```

- [ ] **Step 4: Run focused verification**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::chat_runtime_projection::" -- --nocapture`
Expected: PASS

- [ ] **Step 5: Run broader relevant checks**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::state_lifecycle"`
Expected: PASS with existing `prompt` / `continue` / `snapshot` semantics preserved for legacy callers.

- [ ] **Step 6: Commit**

```bash
git add crates/roci-core/src/agent/runtime.rs \
  crates/roci-core/src/agent/runtime/events.rs \
  crates/roci-core/src/agent/runtime/lifecycle.rs \
  crates/roci-core/src/agent/runtime/run_loop.rs \
  crates/roci-core/src/agent/runtime/state.rs \
  crates/roci-core/src/agent/runtime_tests/support.rs \
  crates/roci-core/src/agent/runtime_tests/chat_runtime_projection.rs

git commit -m "feat: project runtime lifecycle into chat state"
```

### Task 4: Implement `subscribe(cursor)` Replay/Live Stream and Optional Event Store

**Files:**
- Create: `crates/roci-core/src/agent/runtime/chat/subscription.rs`
- Create: `crates/roci-core/src/agent/runtime/chat/store.rs`
- Modify: `crates/roci-core/src/agent/runtime/chat/projector.rs`
- Modify: `crates/roci-core/src/agent/runtime/state.rs:7-107`
- Test: `crates/roci-core/src/agent/runtime_tests/chat_subscription.rs`

**Acceptance criteria:**
- `subscribe(None)` returns a live subscription bound to the runtime’s default thread.
- `subscribe(Some(cursor))` replays semantic events from memory when available, falls back to optional store when configured, and returns `StaleRuntime` when the gap cannot be satisfied.
- `RuntimeEventStore` persists only `AgentRuntimeEvent`, never `AgentEvent` or `RunEvent`.

**Tracking:**
- Tasque child: `tsq-c9dvskta.4`
- Dependencies: `starts_after Task 2`; can run in parallel with `Task 3` after Task 2 lands

- [ ] **Step 1: Write the failing subscription tests**

```rust
use super::support::*;
use crate::agent::runtime::chat::{RuntimeCursor, RuntimeEventStore};

#[tokio::test]
async fn subscribe_replays_gap_then_continues_live() {
    let agent = seeded_runtime_with_completed_turns(2).await;
    let thread = agent.read_snapshot().await.threads[0].clone();
    let mut sub = agent.subscribe(Some(RuntimeCursor { thread_id: thread.thread_id, seq: 2 }));

    let replay = sub.recv().await.expect("item").expect("event");
    assert_eq!(replay.seq, 3);

    let producer = tokio::spawn({
        let agent = agent.clone();
        async move { agent.prompt("live").await.unwrap() }
    });
    let live = sub.recv().await.expect("item").expect("event");
    assert!(live.seq > replay.seq);
    producer.await.expect("join");
}

#[tokio::test]
async fn stale_cursor_returns_error_without_partial_rebuild() {
    let agent = seeded_runtime_with_small_replay_capacity(2).await;
    let thread = agent.read_snapshot().await.threads[0].clone();
    let mut sub = agent.subscribe(Some(RuntimeCursor { thread_id: thread.thread_id, seq: 1 }));

    let err = sub.recv().await.expect("item").expect_err("stale cursor expected");
    assert!(matches!(err, AgentRuntimeError::StaleRuntime { .. }));
    assert!(sub.recv().await.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::chat_subscription::" -- --nocapture`
Expected: FAIL with missing `subscribe` / missing `RuntimeSubscription` / missing replay logic.

- [ ] **Step 3: Implement minimal code**

```rust
// crates/roci-core/src/agent/runtime/chat/store.rs
#[async_trait::async_trait]
pub trait RuntimeEventStore: Send + Sync {
    async fn append(&self, event: AgentRuntimeEvent) -> Result<(), AgentRuntimeError>;
    async fn read_from(
        &self,
        thread_id: ThreadId,
        after_seq: u64,
        limit: usize,
    ) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError>;
}

pub struct MemoryRuntimeEventStore {
    inner: Mutex<HashMap<ThreadId, Vec<AgentRuntimeEvent>>>,
}

// crates/roci-core/src/agent/runtime/chat/subscription.rs
pub struct RuntimeSubscription {
    replay: VecDeque<Result<AgentRuntimeEvent, AgentRuntimeError>>,
    live: broadcast::Receiver<Result<AgentRuntimeEvent, AgentRuntimeError>>,
    closed: bool,
}

impl RuntimeSubscription {
    pub async fn recv(&mut self) -> Option<Result<AgentRuntimeEvent, AgentRuntimeError>> {
        if let Some(item) = self.replay.pop_front() {
            if item.is_err() {
                self.closed = true;
            }
            return Some(item);
        }
        if self.closed {
            return None;
        }
        match self.live.recv().await {
            Ok(item) => {
                if item.is_err() {
                    self.closed = true;
                }
                Some(item)
            }
            Err(broadcast::error::RecvError::Closed) => None,
            Err(broadcast::error::RecvError::Lagged(_)) => Some(Err(AgentRuntimeError::StaleRuntime {
                thread_id: self.bound_thread_id,
                requested_seq: self.last_seen_seq,
                oldest_available_seq: self.oldest_available_seq,
                latest_seq: self.latest_seq,
            })),
        }
    }
}

// crates/roci-core/src/agent/runtime/chat/projector.rs
struct ThreadReplayBuffer {
    events: VecDeque<AgentRuntimeEvent>,
    capacity: usize,
}

impl ChatRuntime {
    pub(super) async fn subscribe(&self, cursor: Option<RuntimeCursor>) -> RuntimeSubscription;
    async fn record_event(&self, event: AgentRuntimeEvent) -> Result<(), AgentRuntimeError> {
        // append to per-thread ring
        // append to optional store
        // broadcast live
    }
}

// crates/roci-core/src/agent/runtime/state.rs
pub fn subscribe(&self, cursor: Option<RuntimeCursor>) -> RuntimeSubscription {
    self.chat.subscribe(cursor)
}
```

- [ ] **Step 4: Run focused verification**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::chat_subscription::" -- --nocapture`
Expected: PASS

- [ ] **Step 5: Run broader relevant checks**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::chat_"`
Expected: PASS for contract/projection/runtime/subscription suites together.

- [ ] **Step 6: Commit**

```bash
git add crates/roci-core/src/agent/runtime/state.rs \
  crates/roci-core/src/agent/runtime/chat/projector.rs \
  crates/roci-core/src/agent/runtime/chat/subscription.rs \
  crates/roci-core/src/agent/runtime/chat/store.rs \
  crates/roci-core/src/agent/runtime_tests/chat_subscription.rs

git commit -m "feat: add chat runtime subscriptions and replay"
```

### Task 5: Implement `cancel_turn` Semantics and Stale Invalidation for Out-of-Band Mutations

**Files:**
- Modify: `crates/roci-core/src/agent/runtime/lifecycle.rs:7-168`
- Modify: `crates/roci-core/src/agent/runtime/mutations.rs:57-138`
- Modify: `crates/roci-core/src/agent/runtime/summary.rs:29-66`
- Modify: `crates/roci-core/src/agent/subagents/launcher.rs:64-141`
- Modify: `crates/roci-core/src/agent/runtime/chat/projector.rs`
- Test: `crates/roci-core/src/agent/runtime_tests/chat_cancellation.rs`

**Acceptance criteria:**
- `cancel_turn(turn_id)` implements queued/running/terminal/stale semantics exactly as requested.
- `abort()` remains compatibility sugar over `cancel_turn(active_turn_id)`.
- `replace_messages`, `compact`, and `reset` mark active subscriptions stale instead of silently mutating replay state.

**Tracking:**
- Tasque child: `tsq-c9dvskta.5`
- Dependencies: `starts_after Task 3`, `starts_after Task 4`

- [ ] **Step 1: Write the failing cancellation and invalidation tests**

```rust
use super::support::*;
use crate::agent::runtime::chat::{RuntimeCursor, TurnStatus};
use std::sync::Arc;
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn cancel_turn_cancels_running_turn_even_if_provider_finishes_racing() {
    let registry = registry_with_racy_provider(Duration::from_millis(50));
    let agent = Arc::new(AgentRuntime::new(registry, test_config(), test_agent_config()));
    let thread_id = agent.read_snapshot().await.threads[0].thread_id;

    let run = {
        let agent = agent.clone();
        tokio::spawn(async move { agent.prompt("race").await })
    };

    wait_for_running_turn(agent.clone()).await;
    let turn_id = agent.read_thread(thread_id).await.unwrap().active_turn_id.unwrap();
    let turn = agent.cancel_turn(turn_id).await.expect("cancel should succeed");
    assert_eq!(turn.status, TurnStatus::Canceled);

    let _ = run.await.expect("join");
    let thread = agent.read_thread(thread_id).await.unwrap();
    assert_eq!(thread.turns.last().unwrap().status, TurnStatus::Canceled);
}

#[tokio::test]
async fn replace_messages_forces_existing_subscription_stale() {
    let agent = seeded_runtime_with_completed_turns(1).await;
    let thread = agent.read_snapshot().await.threads[0].clone();
    let mut sub = agent.subscribe(Some(RuntimeCursor { thread_id: thread.thread_id, seq: thread.last_seq }));

    agent.replace_messages(vec![ModelMessage::user("replacement")]).await.unwrap();

    let err = sub.recv().await.expect("item").expect_err("stale expected");
    assert!(matches!(err, AgentRuntimeError::StaleRuntime { .. }));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::chat_cancellation::" -- --nocapture`
Expected: FAIL because `cancel_turn` does not exist and out-of-band mutations do not invalidate subscriptions.

- [ ] **Step 3: Implement minimal code**

```rust
// crates/roci-core/src/agent/runtime/lifecycle.rs
pub async fn cancel_turn(
    &self,
    turn_id: TurnId,
) -> Result<TurnSnapshot, AgentRuntimeError> {
    match self.chat.request_cancel(turn_id).await? {
        CancelDecision::Queued(turn) => Ok(turn),
        CancelDecision::Running { turn, should_abort } => {
            if should_abort {
                let mut abort_tx = self.active_abort_tx.lock().await;
                if let Some(tx) = abort_tx.take() {
                    let _ = tx.send(());
                }
            }
            Ok(turn)
        }
        CancelDecision::AlreadyTerminal { status } => {
            Err(AgentRuntimeError::AlreadyTerminal { turn_id, status })
        }
    }
}

pub async fn abort(&self) -> bool {
    let Some(turn_id) = self.chat.active_turn_id().await else {
        return false;
    };
    self.cancel_turn(turn_id).await.is_ok()
}

// crates/roci-core/src/agent/runtime/chat/projector.rs
pub(super) async fn request_cancel(&self, turn_id: TurnId) -> Result<CancelDecision, AgentRuntimeError> {
    // revision mismatch => StaleRuntime
    // queued => mark canceled, emit TurnCanceled
    // running => latch canceled, emit TurnCanceled immediately, suppress later completed/failed terminal events
    // terminal => AlreadyTerminal
}

pub(super) async fn invalidate_thread(
    &self,
    thread_id: ThreadId,
    requested_seq: Option<u64>,
) {
    // bump thread revision
    // clear replay ring for thread
    // broadcast Err(StaleRuntime { ... }) to live subscribers
}

// crates/roci-core/src/agent/runtime/mutations.rs
pub async fn replace_messages(&self, messages: Vec<ModelMessage>) -> Result<(), RociError> {
    let thread_id = self.chat.default_thread_id().await;
    self.chat.replace_thread_history(thread_id, messages).await?;
    self.sync_legacy_watchers_from_chat().await;
    Ok(())
}

// crates/roci-core/src/agent/runtime/summary.rs
if let Some(compacted_messages) = compacted {
    self.chat.replace_thread_history(self.chat.default_thread_id().await, compacted_messages).await?;
    self.sync_legacy_watchers_from_chat().await;
}

// crates/roci-core/src/agent/subagents/launcher.rs
if !initial_messages.is_empty() {
    runtime.bootstrap_initial_thread_messages(initial_messages).await?;
}
```

- [ ] **Step 4: Run focused verification**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::chat_cancellation::" -- --nocapture`
Expected: PASS

- [ ] **Step 5: Run broader relevant checks**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::compaction_and_branch_summary"`
Expected: PASS with manual compaction still functional and now forcing stale-resync behavior for subscribers.

- [ ] **Step 6: Commit**

```bash
git add crates/roci-core/src/agent/runtime/lifecycle.rs \
  crates/roci-core/src/agent/runtime/mutations.rs \
  crates/roci-core/src/agent/runtime/summary.rs \
  crates/roci-core/src/agent/runtime/chat/projector.rs \
  crates/roci-core/src/agent/subagents/launcher.rs \
  crates/roci-core/src/agent/runtime_tests/chat_cancellation.rs

git commit -m "feat: add semantic turn cancellation"
```

### Task 6: Migrate Legacy Accessors/Tests/Docs/Examples onto Chat Runtime Semantics

**Files:**
- Modify: `crates/roci-core/src/agent/runtime_tests/support.rs:112-145`
- Modify: `crates/roci-core/src/agent/runtime_tests/snapshot.rs:1-97`
- Modify: `crates/roci-core/src/agent/runtime_tests/state_lifecycle.rs:1-320`
- Modify: `crates/roci-core/src/agent/runtime_tests/queue_and_continue.rs:1-74`
- Modify: `crates/roci-core/src/agent/runtime_tests/compaction_and_branch_summary.rs:1-156`
- Modify: `crates/roci-cli/src/chat.rs:90-128`
- Modify: `examples/agent_runtime.rs:1-190`
- Modify: `docs/ARCHITECTURE.md:114-129`

**Acceptance criteria:**
- Existing shallow APIs (`state()`, `watch_state()`, `snapshot()`, `watch_snapshot()`, `messages()`, `abort()`) still behave sensibly, but are documented as compatibility surfaces over chat state.
- Architecture docs describe the three layers: `agent_loop`, `runtime::chat`, host app.
- Example code demonstrates snapshot/read-thread/subscription flow and clarifies that `abort()` is sugar for `cancel_turn`.

**Tracking:**
- Tasque child: `tsq-c9dvskta.6`
- Dependencies: `starts_after Task 3`, `starts_after Task 4`, `starts_after Task 5`

- [ ] **Step 1: Write the failing compatibility/doc tests**

```rust
#[tokio::test]
async fn legacy_snapshot_still_matches_default_thread_summary() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    let shallow = agent.snapshot().await;
    let runtime = agent.read_snapshot().await;
    let thread = &runtime.threads[0];

    assert_eq!(shallow.turn_index, thread.turns.len());
    assert_eq!(shallow.message_count, thread.messages.len());
    assert_eq!(shallow.is_streaming, thread.active_turn_id.is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::snapshot"`
Expected: FAIL until legacy tests are updated to read from chat-backed state.

- [ ] **Step 3: Implement minimal code**

```rust
// crates/roci-core/src/agent/runtime_tests/support.rs
AgentConfig {
    // existing fields ...
    chat: ChatRuntimeConfig::default(),
}

// crates/roci-cli/src/chat.rs
AgentConfig {
    // existing fields ...
    chat: ChatRuntimeConfig::default(),
}

// examples/agent_runtime.rs
let runtime_snapshot = agent.read_snapshot().await;
let thread = agent.read_thread(runtime_snapshot.threads[0].thread_id).await.unwrap();
let mut sub = agent.subscribe(None);
// document: abort() remains sugar, prefer cancel_turn(thread.active_turn_id.unwrap())

// docs/ARCHITECTURE.md runtime section
- `agent_loop` owns provider/tool lifecycle only.
- `agent::runtime::chat` owns threads, semantic turns, message/tool projection, replay buffer, and cancellation semantics.
- Hosts own transport/auth/storage/UI mapping and should reconnect via `read_snapshot()` + `subscribe(cursor)`.
```

- [ ] **Step 4: Run focused verification**

Run: `cargo test -p roci-core --features agent "agent::runtime::tests::"`
Expected: PASS

- [ ] **Step 5: Run broader relevant checks**

Run: `cargo fmt --all`
Expected: PASS

Run: `cargo clippy -p roci-core --features agent --tests -- -D warnings`
Expected: PASS

Run: `cargo test -p roci-cli`
Expected: PASS

Run: `cargo test`
Expected: PASS

Run:
```bash
curl http://127.0.0.1:1234/api/v0/models
tmux new-session -d -s roci-chat-runtime \
  'cd /Users/adityasharma/Projects/roci && \
   LMSTUDIO_BASE_URL=http://127.0.0.1:1234 \
   cargo run -q -p roci-cli --features roci/agent,roci/lmstudio -- \
   chat --no-skills --model "lmstudio:<loaded-model-id>" \
   "Reply exactly: roci-runtime-chat-ok"; \
   status=$?; printf "\n[roci live exit=%s]\n" "$status"; exec zsh'
echo "attach: tmux attach -t roci-chat-runtime"
```
Expected: provider returns `roci-runtime-chat-ok`; tmux attach command is shown before completion. If no local model is loaded, note that explicitly and rerun with the configured remote provider most relevant to the change.

- [ ] **Step 6: Commit**

```bash
git add crates/roci-core/src/agent/runtime_tests/support.rs \
  crates/roci-core/src/agent/runtime_tests/snapshot.rs \
  crates/roci-core/src/agent/runtime_tests/state_lifecycle.rs \
  crates/roci-core/src/agent/runtime_tests/queue_and_continue.rs \
  crates/roci-core/src/agent/runtime_tests/compaction_and_branch_summary.rs \
  crates/roci-cli/src/chat.rs \
  examples/agent_runtime.rs \
  docs/ARCHITECTURE.md

git commit -m "docs: align runtime surfaces with chat semantics"
```

---

## Migration / Compatibility Strategy

1. **Keep low-level `AgentEvent` sink intact.** `AgentConfig.event_sink` remains for CLI/demo code and any existing host that wants tool/provider lifecycle deltas. New reconnect-capable hosts should move to `read_snapshot()` + `subscribe()`.
2. **Keep shallow snapshot/state APIs as adapters.** `state()`, `watch_state()`, `snapshot()`, `watch_snapshot()`, `messages()`, and `abort()` stay callable, but they become compatibility views over the chat projector instead of owning state.
3. **Stop treating `result.messages` as the first state commit.** It becomes only a terminal consistency check and return value. Incremental state lives in the projector during the run.
4. **Do not emit `SnapshotUpdated`.** Out-of-band history rewrites (`replace_messages`, `compact`, `reset`) instead invalidate replay and force callers to resync with `read_snapshot()`.
5. **Do not make persistence mandatory.** The ring buffer satisfies reconnect within a bounded window; the optional `RuntimeEventStore` only extends replay depth.
6. **Keep single-thread runtime scope explicit.** `ThreadId` is present everywhere now, but `AgentRuntime` still manages one default thread in v1. That keeps public contracts future-proof without dragging in branch/switch APIs.

## Risks / Tradeoffs

- **Single default thread per runtime**: smallest scope, cleanest migration from today. Tradeoff: `subscribe(None)` semantics are only obvious because there is one thread. If product scope wants true multi-thread-per-runtime now, add a separate contract task before coding.
- **`TurnCanceled` wins over late provider success/failure**: this matches the requested cancellation semantics and avoids host-visible races. Tradeoff: a provider response that lands after the cancel latch is intentionally discarded from semantic terminal state.
- **Out-of-band mutations force `StaleRuntime`**: this avoids inventing synthetic snapshot events and keeps incremental semantics honest. Tradeoff: callers doing manual compaction/reset/replacement must be ready to full-resync.
- **Two event layers coexist**: `AgentEvent` for low-level lifecycle, `AgentRuntimeEvent` for semantic chat state. Tradeoff: some temporary duplication, but the layering stays correct and keeps `agent_loop` ignorant of reconnect/UI concerns.
- **Thread-scoped seq, no global seq**: matches the requested cursor contract and avoids false cross-thread ordering promises. Tradeoff: a future multiplexed multi-thread host would need either one subscription per thread or a new connection-level ordering contract.

## Verification Gates

- New runtime contract tests: `chat_contracts`, `chat_projection`, `chat_runtime_projection`, `chat_subscription`, `chat_cancellation`.
- Existing runtime regression suite still green: `cargo test -p roci-core --features agent "agent::runtime::tests::"`.
- Lint/format gate: `cargo fmt --all`, `cargo clippy -p roci-core --features agent --tests -- -D warnings`.
- Consumer compile gate: `cargo test -p roci-cli`.
- Workspace regression gate: `cargo test`.
- Live tmux/provider smoke gate: `roci-cli chat` with attach command printed up front.

## Parallel Workstreams

- After **Task 2**, run **Task 3** and **Task 4** in parallel.
- **Task 5** starts after both lifecycle wiring and subscription logic land.
- **Task 6** can begin once `Task 3` is merged for compile fixes, but it should finish only after `Task 5` so docs/tests reflect stale invalidation and cancel semantics.

## Open Question / Confirmation

- Confirm the v1 scope assumption: **one `ThreadSnapshot` per `AgentRuntime` instance**, thread-aware types now, no in-runtime thread creation/switch API yet. If you want multi-thread-per-runtime in this same implementation, expand the plan before coding; it changes `subscribe(None)`, queue ownership, and host API shape materially.
