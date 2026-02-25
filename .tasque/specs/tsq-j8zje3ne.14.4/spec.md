# abort/reset/wait_for_idle Lifecycle Tests

## Goal
Stress lifecycle control methods under concurrent and edge timing scenarios.

## Scope
- Abort during active run.
- Reset while idle and while running.
- wait_for_idle behavior across success/error/cancel.

## Acceptance Criteria
- No deadlocks/hangs.
- Agent remains reusable after each lifecycle path.
