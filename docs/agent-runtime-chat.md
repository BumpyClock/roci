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
  - `RuntimeSubscription` events for replay + live updates
  - transport, auth, storage, and UI concerns outside `roci-core`

## Public APIs

- `read_snapshot() -> RuntimeSnapshot` (async)
  - Returns all in-memory projected threads, including `thread_id`, `revision`,
    and `last_seq`.
- `read_thread(thread_id: ThreadId) -> Result<ThreadSnapshot, AgentRuntimeError>` (async)
  - Returns one thread projection.
  - `Err(ThreadNotFound)` when the thread id is unknown.
- `subscribe(cursor: Option<RuntimeCursor>) -> RuntimeSubscription` (async)
  - `None`: subscribe only to live semantic runtime events.
  - `Some(cursor)`: replay retained events for that thread cursor, then receive live
    events from `recv`/`next`.
- `cancel_turn(turn_id: TurnId) -> Result<TurnSnapshot, AgentRuntimeError>` (async)
  - Cancels queued/running turns.
  - Returns `AlreadyTerminal` for completed/failed/canceled turns.
  - Returns `StaleRuntime` when the `turn_id` revision is not current (history reset/rewrite).
- `abort()` remains as compatibility sugar:
  - Resolves active `turn_id` and calls `cancel_turn` when possible.
  - Falls back to legacy abort path when no active turn is currently projected.

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

`RuntimeSnapshot` also has `schema_version` so hosts can version full snapshot
sync separately from individual event cursors.

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
   - call `read_snapshot()` and update local state.
2. Incremental resume:
   - keep last seen `RuntimeCursor` (`thread_id`, `seq`)
   - call `subscribe(Some(cursor))`
   - process `subscription.replay()` first, then `subscription.recv()` live stream
3. Stale cursor:
   - if `StaleRuntime` is returned, host must resync with `read_snapshot()` and
     derive a fresh cursor, then re-subscribe.
