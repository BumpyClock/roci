# Implement steer/follow_up/abort/reset

## Goal
Complete mutating control APIs and lifecycle teardown semantics.

## Scope
- Implement `steer` and `follow_up` queue wiring.
- Implement `abort` signaling to active run.
- Implement `reset` state + conversation/session cleanup semantics.
- Add tests for race scenarios (abort while tools running, reset while idle/running).

## Acceptance Criteria
- Steering and follow-up messages are consumed at documented checkpoints.
- Abort is idempotent and leaves agent recoverable.
- Reset returns agent to clean idle baseline.
