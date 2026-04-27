# AgentRuntime Chat Projection Semantics Plan

## Goal
Roci owns runtime semantics for chat/agent runs without forcing event-sourced persistence. Host apps own transport, auth, persistence policy, and UI protocol mapping. Homie should map Roci snapshots/events directly to JSON-RPC/WebSocket with no raw `AgentEvent` replay or custom turn projector.

## Layering

```text
agent_loop
  low-level execution, provider/tool lifecycle

agent::runtime::chat
  domain projection: runtime/thread/turn/message/tool state, cancellation, subscription cursor

host app
  transport, auth, persistence backend/policy, UI protocol mapping
```

`agent_loop` stays ignorant of web/mobile/reconnect semantics. `RuntimeEventStore` stores semantic `AgentRuntimeEvent` only; never raw `AgentEvent` or `RunEvent`.

## V1 Scope Decisions
- One stable default thread per `AgentRuntime`.
- Snapshot/event types are thread-aware now; multi-thread creation/switch APIs stay out of scope.
- `read_snapshot()` returns `RuntimeSnapshot` and is full-sync truth for reconnect.
- `subscribe(cursor)` returns incremental semantic events; clients are not required to rebuild state from events.
- Default replay is in-memory bounded buffer, `512` semantic events per thread.
- Optional `RuntimeEventStore` extends replay depth only; snapshot state is never rebuilt from store in v1.
- No `SnapshotUpdated` event.
- No `MessageFailed` until message streaming can fail independently from turn failure.
- `RuntimeCursor = { thread_id, seq }`; seq scoped per thread.
- Semantic chat turn is runtime-owned and distinct from low-level `agent_loop::TurnStart/TurnEnd` loop iterations.

## Target Modules

```text
crates/roci-core/src/agent/runtime/chat/
  mod.rs
  domain.rs        // RuntimeSnapshot, ThreadSnapshot, TurnSnapshot, MessageSnapshot, ids, statuses
  event.rs         // RuntimeCursor, AgentRuntimeEvent envelope + semantic payloads
  projector.rs     // ordered AgentEvent -> chat state/runtime events
  subscription.rs  // subscribe(cursor), bounded replay + live stream
  store.rs         // optional RuntimeEventStore trait + in-memory impl
  error.rs         // AgentRuntimeError
```

## Public API Target

```rust
impl AgentRuntime {
    pub async fn read_snapshot(&self) -> RuntimeSnapshot;
    pub async fn read_thread(&self, thread_id: ThreadId) -> Result<ThreadSnapshot, AgentRuntimeError>;
    pub fn subscribe(&self, cursor: Option<RuntimeCursor>) -> RuntimeSubscription;
    pub async fn cancel_turn(&self, turn_id: TurnId) -> Result<TurnSnapshot, AgentRuntimeError>;
}
```

`abort()` remains compatibility sugar. It must continue driving legacy `AgentState::Aborting`, `watch_state()`, `watch_snapshot()`, and `wait_for_idle()` semantics while delegating active-turn cancellation to `cancel_turn` internally where possible.

## Domain Contracts

Statuses:
```rust
pub enum TurnStatus { Queued, Running, Completed, Failed, Canceled }
pub enum MessageStatus { Streaming, Completed }
pub enum ToolStatus { Running, Completed }
```

Errors:
```rust
pub enum AgentRuntimeError {
    RuntimeBusy,
    ThreadNotFound { thread_id: ThreadId },
    TurnNotFound { turn_id: TurnId },
    AlreadyTerminal { turn_id: TurnId, status: TurnStatus },
    StaleRuntime { thread_id: ThreadId, requested_seq: u64, oldest_available_seq: u64, latest_seq: u64 },
    ProjectionFailed { message: String },
}
```

Event envelope:
```rust
pub struct RuntimeCursor {
    pub thread_id: ThreadId,
    pub seq: u64,
}

pub struct AgentRuntimeEvent {
    pub schema_version: u16,
    pub seq: u64,
    pub thread_id: ThreadId,
    pub turn_id: Option<TurnId>,
    pub timestamp: DateTime<Utc>,
    pub payload: AgentRuntimeEventPayload,
}
```

Event payload set:
```text
TurnQueued
TurnStarted
MessageStarted
MessageUpdated
MessageCompleted
ToolStarted
ToolUpdated
ToolCompleted
TurnCompleted
TurnFailed
TurnCanceled
```

## Critical Design Fixes From Plan Review

### Provider Context vs Turn Input
Current `prompt()`/`continue_run()` build a full provider context snapshot. The projector must not treat that full vector as the new semantic turn.

Implementation must split:
```text
turn_input_messages       // only new user/queued messages for semantic turn
provider_context_messages // full LLM context sent to agent_loop
```

Add explicit bootstrap/import API for existing history:
```rust
ChatProjector::bootstrap_thread(messages: Vec<ModelMessage>) -> Result<ThreadSnapshot, AgentRuntimeError>
```

Used by `replace_messages`, subagent child seeding, compaction output, and tests. Bootstrap/import must preserve exact history and never emit a fresh `TurnQueued` for old transcript.

### Real Queued Cancellation Window
`Queued -> Canceled` with no provider call requires a runtime-owned dispatch gate. `queue_turn(...); run_loop(...).await` immediately is insufficient.

Add a start gate:
```text
queue semantic turn -> emit TurnQueued -> check cancellation -> transition Running -> start provider run
```

Acceptance: queued cancel test proves provider invocation count stays zero.

### Ordered Projection Lane
Do not project `AgentEvent` via detached `tokio::spawn`. Projection must be serialized per thread/run.

Options:
- Synchronous projector lock in intercepting sink with no await, if projection stays non-async.
- Or bounded `mpsc` event lane drained by one task, with terminal run awaiting lane flush before final snapshot/result.

Acceptance: monotonic seq under streaming deltas, parallel tool completions, steering/follow-up.

### Store/Broadcast Ordering
Append semantic event to in-memory buffer/store before live broadcast. Cursor replay must never see gaps/duplicates caused by broadcast-before-store.

Contract:
```text
project state -> allocate seq -> append replay buffer/store -> broadcast live event
```

Store failures for optional external store must be surfaced as projection/runtime errors, not silently ignored.

### Stale Runtime Invalidations
Out-of-band state rewrites invalidate incremental subscribers and stale ids/cursors:
- `replace_messages`
- manual `compact`
- `reset`
- subagent/bootstrap import when replacing child history

Acceptance: all four covered. `reset()` explicitly included.

## Implementation Tasks

### tsq-c9dvskta.1 Define chat runtime contracts and config seam
Files: create `chat/{mod,domain,event,error}.rs`; update `runtime.rs`, `runtime/config.rs`, `agent/mod.rs`.
Acceptance:
- Public exports compile.
- `ChatRuntimeConfig::default()` has `replay_capacity = 512`, no external store.
- IDs carry thread/revision for stale detection.
- Contract tests cover event payload names, cursor shape, error display.

### tsq-c9dvskta.2 Implement in-memory chat projection state and default thread model
Files: `chat/projector.rs`, `chat/store.rs`.
Acceptance:
- `RuntimeSnapshot` / `ThreadSnapshot` generated from in-memory state.
- Per-thread seq monotonic.
- Event append order = state update -> buffer/store -> broadcast-ready event.
- Pure projector tests for message/tool/turn state.

### tsq-c9dvskta.7 Define and implement history bootstrap/import projection seam
Files: `chat/projector.rs`, `runtime/mutations.rs`, subagent seed call sites.
Acceptance:
- `replace_messages()` and child seeding round-trip exact history into `read_thread()` and legacy `messages()`.
- No duplicated old transcript in new semantic turn.
- Bootstrap/import invalidates prior subscription cursors with `StaleRuntime`.

### tsq-c9dvskta.3 Wire AgentRuntime lifecycle and ordered AgentEvent projection
Files: `runtime/events.rs`, `runtime/lifecycle.rs`, `runtime/run_loop.rs`, `runtime/state.rs`.
Acceptance:
- Split `turn_input_messages` from `provider_context_messages`.
- Dispatch gate emits `TurnQueued`, allows queued cancel, then emits `TurnStarted` before provider call.
- Ordered projection lane handles `AgentEvent` deterministically.
- `read_snapshot()` reflects in-flight assistant/tool state before run completes.
- Terminal result flushes projection lane before `wait_for_idle()` resolves.

### tsq-c9dvskta.4 Implement subscribe(cursor) replay/live stream and optional event store
Files: `chat/subscription.rs`, `chat/store.rs`, runtime re-exports.
Acceptance:
- `subscribe(Some(cursor))` replays events after cursor, then live events.
- `subscribe(None)` starts from latest live position; full state comes from `read_snapshot()`.
- Lag/gap returns `StaleRuntime` with requested/oldest/latest seq.
- Optional store extends replay only and never rebuilds current snapshot.

### tsq-c9dvskta.5 Implement cancel_turn semantics and stale invalidation
Files: `runtime/lifecycle.rs`, `runtime/run_loop.rs`, `runtime/mutations.rs`, `runtime/summary.rs`.
Acceptance:
- Queued cancel emits `TurnCanceled`, no provider call.
- Running cancel sends abort and emits `TurnCanceled` even if provider races success/failure.
- Late provider success/failure after cancel cannot overwrite semantic `Canceled` terminal state.
- Terminal cancel returns `AlreadyTerminal`.
- Stale revision/id returns `StaleRuntime`.
- `abort()` compatibility: `AgentState::Aborting`, legacy shallow snapshot, and `wait_for_idle()` behavior preserved.

### tsq-c9dvskta.6 Migrate legacy accessors/tests/docs/examples
Files: existing runtime tests, `docs/ARCHITECTURE.md`, `docs/testing.md`, examples, CLI compile fixes.
Acceptance:
- Legacy `snapshot()`, `watch_snapshot()`, `state()`, `messages()` derive from chat truth.
- Docs state host contract: snapshots + semantic runtime events, not raw `AgentEvent` replay.
- Examples show `read_snapshot`, `read_thread`, `subscribe`, `cancel_turn`.
- No provider-facing claim complete until live tmux/provider smoke per `docs/testing.md`.

## Dependencies
- `1 blocks 2`
- `2 blocks 7`
- `7 blocks 3`
- `3 blocks 4`
- `4 blocks 5`
- `5 blocks 6`

This is more serial than first draft because correctness depends on bootstrap, ordered projection, and subscription ordering before cancellation/docs.

## Tests To Add

Runtime contracts:
- `chat_runtime_config_defaults_to_bounded_in_memory_replay`
- `runtime_cursor_is_thread_scoped`
- `turn_and_message_ids_carry_thread_revision`
- `semantic_payload_set_matches_target_contract`

Projection:
- `projector_allocates_monotonic_per_thread_seq`
- `projector_projects_message_lifecycle_without_snapshot_updated`
- `projector_projects_tool_lifecycle`
- `projector_rejects_out_of_order_message_completion`

Bootstrap/import:
- `replace_messages_bootstraps_exact_history_without_duplicate_turn`
- `subagent_child_seed_bootstraps_exact_history`
- `reset_invalidates_runtime_cursors`

Runtime wiring:
- `prompt_emits_turn_queued_before_turn_started`
- `read_snapshot_includes_in_flight_assistant_message`
- `parallel_tool_updates_preserve_monotonic_subscription_order`
- `steering_and_followup_preserve_semantic_turn_order`

Subscription:
- `subscribe_cursor_replays_then_streams_live_events`
- `subscribe_cursor_gap_returns_stale_runtime`
- `external_store_extends_replay_without_rebuilding_snapshot`
- `store_append_happens_before_live_broadcast`

Cancellation:
- `cancel_queued_turn_emits_canceled_and_never_calls_provider`
- `cancel_running_turn_wins_over_late_provider_success`
- `cancel_terminal_turn_returns_already_terminal`
- `abort_preserves_legacy_aborting_state_and_wait_for_idle`

Host contract:
- `host_can_render_thread_from_read_snapshot_without_raw_event_replay`

## Verification Gates
- `cargo fmt --all`
- `cargo clippy -p roci-core --features agent --tests -- -D warnings`
- `cargo test -p roci-core --features agent "agent::runtime::tests::chat_"`
- `cargo test -p roci-core --features agent "agent::runtime::tests::"`
- `cargo test -p roci-cli`
- `cargo test`
- If provider-facing behavior changed: live tmux/provider smoke, attach command shown first.

## Risks / Watchpoints
- `turn_index` currently means low-level loop iteration. Keep legacy value or explicitly document new semantic mapping before changing tests.
- Late provider result after cancel must not leave `messages()` or `RunResult` contradicting semantic canceled state.
- Subscription cursor design is v1 single-thread friendly; revisit if true multi-thread runtime arrives.
- External store failures need a clear policy: fail projection/run vs degrade replay. Default: fail loudly for configured store, since silent persistence gaps break reconnect.
