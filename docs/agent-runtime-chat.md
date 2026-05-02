# Agent runtime chat semantics

Roci's `agent::runtime::chat` layer is the stable public contract for UI/host-facing
runtime state and events.

## Layering

- `agent_loop` emits low-level `crate::agent_loop::AgentEvent` while running provider
  turns and tool execution.
- `agent::runtime::chat` projects those loop events into stable semantic snapshots
  and events (turn/message/tool lifecycle, approvals, reasoning, plan/diff updates,
  turn completion/error/cancel).
- Host apps should consume:
  - `RuntimeSnapshot` / `ThreadSnapshot` for state sync and rendering
  - `AgentRuntimeEvent` through `RuntimeSubscription` for replay + live updates
  - transport, auth, storage, and UI concerns outside `roci-core`

Hosts should not consume raw `agent_loop` events as chat state, maintain a
shadow turn queue, synthesize plans from assistant text, or persist host shims as
runtime state.

## Public APIs

- `read_snapshot() -> RuntimeSnapshot` (async)
  - Returns all in-memory projected threads, including `thread_id`, `revision`,
    and `last_seq`.
- `read_thread(thread_id: ThreadId) -> Result<ThreadSnapshot, AgentRuntimeError>` (async)
  - Returns one thread projection.
  - `Err(ThreadNotFound)` when the thread id is unknown.
- `default_thread_id() -> ThreadId`
  - Returns the runtime-owned default thread id used for queued turns.
  - `ChatRuntimeConfig::default_thread_id` may set this id during construction.
- `subscribe(cursor: Option<RuntimeCursor>) -> RuntimeSubscription` (async)
  - `None`: subscribe only to live semantic runtime events.
  - `Some(cursor)`: replay retained events for that thread cursor, then receive live
    events from `recv`/`next`.
- `import_thread(imported: ImportedThread) -> Result<(), RociError>` (async)
  - Imports a full semantic `ThreadSnapshot`.
  - Replaces provider context from `ImportedThread::model_messages`.
  - Invalidates retained replay at `imported.thread.last_seq`.
  - Requires idle runtime state.
- `enqueue_turn(request: EnqueueTurnRequest) -> Result<TurnId, RociError>` (async)
  - Returns a stable `TurnId` after semantic queue projection and before provider
    execution starts.
  - Runtime serializes queued turns so only one provider run is active at a time.
- `set_generation_settings(settings: GenerationSettings) -> Result<(), RociError>` (async)
  - Idle-only default update for later turns.
- `set_approval_policy(policy: ApprovalPolicy) -> Result<(), RociError>` (async)
  - Idle-only default update for later turns.
- `cancel_turn(turn_id: TurnId) -> Result<TurnSnapshot, AgentRuntimeError>` (async)
  - Cancels queued/running turns.
  - Returns `AlreadyTerminal` for completed/failed/canceled turns.
  - Returns `StaleRuntime` when the `turn_id` revision is not current (history reset/rewrite).
  - Returns `TurnNotFound` or `ThreadNotFound` when the id is unknown.
- `abort()`:
  - Resolves active `turn_id` and calls `cancel_turn` when possible.
  - Uses the runtime abort path when no active turn is currently projected.

## Event contract

All semantic runtime events are in `agent::runtime::chat::AgentRuntimeEvent` and are
wrapped by:

- `schema_version`
- monotonic `seq`
- `thread_id`
- optional `turn_id`
- `timestamp`
- `payload`

`RuntimeCursor` is `(thread_id, seq)` and is emitted per event via `event.cursor()`.

Payloads:
- `turn_queued`
- `turn_started`
- `message_started`
- `message_updated`
- `message_completed`
- `tool_started`
- `tool_updated`
- `tool_completed`
- `approval_required`
- `approval_resolved`
- `approval_canceled`
- `human_interaction_requested`
- `human_interaction_resolved`
- `human_interaction_canceled`
- `reasoning_updated`
- `plan_updated`
- `diff_updated`
- `turn_completed`
- `turn_failed`
- `turn_canceled`

No `SnapshotUpdated` (or raw `AgentEvent`) payload is part of this public contract.

Approval, reasoning, plan, and diff payloads carry runtime-owned snapshots:

- `ApprovalSnapshot`
  - request, status (`pending`, `resolved`, `canceled`), decision, timestamps
- `HumanInteractionSnapshot`
  - request, status (`pending`, `resolved`, `canceled`), response/error, timestamps
- `ReasoningSnapshot`
  - `turn_id`, optional `message_id`, accumulated text snapshot
  - `reasoning_updated` also includes the incremental `delta`
- `PlanSnapshot`
  - latest plan text for the turn
- `DiffSnapshot`
  - latest diff text for the turn

Plan/diff updates are semantic runtime inputs. When the loop or host integration
emits `AgentEvent::PlanUpdated` / `AgentEvent::DiffUpdated`, chat projection owns
the stable event/store/replay shape; hosts still consume only `AgentRuntimeEvent`.

`ThreadSnapshot` includes projected `approvals`, `reasoning`, `plans`, and `diffs`
so reconnecting hosts can recover current semantic state without reading raw loop
events.

Approval status contract:

- `pending`: host decision has been requested
- `resolved`: host returned an approval decision other than cancel
- `canceled`: host returned cancel; this is distinct from declined

Human interaction contract:

- Raw loop events use `AgentEvent::HumanInteractionRequested`,
  `AgentEvent::HumanInteractionResolved`, and `AgentEvent::HumanInteractionCanceled`.
- `ask_user` is represented as an `AskUser` payload in the human interaction envelope.
- Hosts render pending `HumanInteractionSnapshot`s and resolve them through the runtime or shared coordinator by `request_id`.

`RuntimeSnapshot` also has `schema_version` so hosts can version full snapshot
sync separately from individual event cursors.

## Host reconnect contract

Full reconnect uses:

```rust
pub struct ImportedThread {
    pub thread: ThreadSnapshot,
    pub model_messages: Vec<ModelMessage>,
}
```

- `thread` is the semantic UI/runtime snapshot. Roci preserves its `revision`,
  `last_seq`, `active_turn_id`, turns, messages, tools, approvals, reasoning,
  plans, diffs, and human interactions.
- `model_messages` is the provider context ledger. Roci uses only this ledger for
  the next provider request context.
- `read_thread(imported.thread.thread_id)` after import returns the imported
  semantic snapshot.
- Imported active/running turns preserve semantic state only. Provider execution
  is not resurrected.
- Host apps should set `AgentConfig.chat.default_thread_id` when reconnecting to
  a known thread so queued turns continue on that thread.

Incremental updates use `AgentRuntimeEvent` only. `ThreadSnapshot` remains the
semantic view/runtime state; `model_messages` remains provider context.

## Queue contract

Roci owns queued-turn semantics:

```rust
pub enum CollaborationMode {
    Code,
    Plan,
}

pub struct EnqueueTurnRequest {
    pub messages: Vec<ModelMessage>,
    pub generation_settings: Option<GenerationSettings>,
    pub approval_policy: Option<ApprovalPolicy>,
    pub collaboration_mode: Option<CollaborationMode>,
}
```

- `CollaborationMode::Code` is the default current behavior.
- `enqueue_turn` freezes effective generation settings and approval policy for
  that turn. Per-turn overrides beat runtime defaults.
- Queued turns emit normal semantic events through the projector, event store,
  and subscriptions.
- `cancel_turn(turn_id)` cancels queued turns before provider start. Running
  turns use the existing abort path.
- Queue execution remains single active provider run at a time.

`CollaborationMode::Plan` uses a structured response contract. Runtime emits
`AgentRuntimeEventPayload::PlanUpdated` for plan mode only from parsed structured
plan data, not from text scraping. If the provider returns malformed structured
plan output, the turn fails instead of synthesizing a plan from prose.

## Cancellation semantics

- Turn status set:
  - `queued`
  - `running`
  - `completed`
  - `failed`
  - `canceled`
- `cancel_turn` may be called on queued or running turns.
  - terminal turns (completed/failed/canceled) return `AlreadyTerminal`
  - stale ids (old revision / replaced snapshot history) return `StaleRuntime`
- On successful cancel:
  - pending approvals for the turn are emitted as `approval_canceled`
  - a `TurnCanceled` event is emitted
  - terminal projection updates become visible in snapshots

## Persistence contract

- `AgentConfig::chat.event_store` provides the semantic event store.
- If omitted, runtime constructor uses an in-memory store:
  - `InMemoryAgentRuntimeEventStore`
  - bounded by replay capacity (default: `ChatRuntimeConfig::default().replay_capacity = 512`)
- Event store contract is only for replaying semantic `AgentRuntimeEvent`.
- Raw `AgentEvent` stream is not intended as public persistence/replay API.

## Reconnect flow

1. Full sync:
   - call `import_thread(ImportedThread { thread, model_messages })` when
     restoring saved host state, or call `read_snapshot()` to render current
     runtime state.
2. Incremental resume:
   - keep last seen `RuntimeCursor` (`thread_id`, `seq`)
   - call `subscribe(Some(cursor))`
   - process `subscription.replay()` first, then `subscription.recv()` live stream
3. Stale cursor:
   - if `StaleRuntime` is returned, host must resync with `read_snapshot()` and
     derive a fresh cursor, then re-subscribe.
