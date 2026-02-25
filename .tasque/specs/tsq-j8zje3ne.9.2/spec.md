# Implement prompt/continue/wait_for_idle

## Goal
Wire agent APIs that initiate/continue runs and expose deterministic idle waiting.

## Scope
- Implement `prompt` and `continue_run` using loop runner.
- Persist and update active run tracking.
- Implement `wait_for_idle` for both success and cancellation outcomes.

## Acceptance Criteria
- Starting a run transitions state to running and emits expected events.
- `wait_for_idle` resolves exactly once per run completion path.
- Re-entrant start behavior is defined and tested.
