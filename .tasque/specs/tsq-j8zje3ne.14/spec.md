# Agent Runtime Test Matrix

## Goal
Add complete test coverage for new runtime behaviors and regression-prone paths.

## Coverage Buckets
1. Event ordering/lifecycle (`AgentStart/TurnStart/.../AgentEnd`).
2. Steering + follow-up control-flow correctness.
3. Tool execution behavior (parallel/sequential/skip/validation errors).
4. Abort/reset/wait-for-idle lifecycle edge cases.

## Files
- `src/agent_loop/runner.rs` (loop-focused tests)
- `src/agent/agent.rs` (agent API tests)
- `tests/` integration tests if broader black-box scenarios needed

## Acceptance Criteria
- Deterministic assertions on event sequence.
- Regression tests for known failure modes (stuck run, duplicate tool completion, lost steering message).
- No flaky timing-based tests (use deterministic channels/mocks).
