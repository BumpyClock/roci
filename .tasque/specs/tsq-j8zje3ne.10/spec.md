# Observable Agent State + Event Subscription

## Goal
Expose observable agent state and subscription API suitable for UIs and orchestration layers.

## Scope
- Add state snapshot struct (run status, turn index, current message/tool metadata).
- Add subscribe/unsubscribe listener mechanism.
- Emit updates on lifecycle, message, and tool events.
- Add tests for ordering and listener cleanup.

## Files
- `src/agent/agent.rs`
- `src/agent_loop/events.rs` (if additional mapped fields needed)

## Acceptance Criteria
- Multiple listeners receive identical ordered updates.
- Listener removal prevents further callbacks.
- No deadlocks/panics when listener callback is slow or fails.
