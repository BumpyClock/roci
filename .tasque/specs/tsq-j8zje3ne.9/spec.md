# Agent Public API Runtime Integration

## Goal
Refactor `Agent` to orchestrate loop runner with explicit APIs:
`prompt`, `continue_run`, `steer`, `follow_up`, `abort`, `reset`, `wait_for_idle`.

## Scope
- Define `Agent` internal runtime state machine (`idle/running/aborting`).
- Wire queued steering/follow-up input channels to loop hooks.
- Keep backward-compatible convenience APIs where feasible.
- Add unit/integration tests for lifecycle operations.

## Files
- `src/agent/agent.rs`
- `src/agent/mod.rs`
- `src/agent_loop/runner.rs` (integration points only)

## Acceptance Criteria
- API methods are race-safe and deterministic under concurrent calls.
- `abort` cancels active run and transitions to idle.
- `reset` clears transient state and conversation/session state as specified.
- `wait_for_idle` resolves on both success and abort paths.
