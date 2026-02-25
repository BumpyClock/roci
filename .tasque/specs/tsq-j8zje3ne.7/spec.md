# transform_context Hook

## Goal
Validate async pre-LLM context transformation hook behavior and ordering.

## Scope
- Add tests that install `transform_context` hook.
- Verify hook executes before each provider call.
- Verify transformed messages are what provider receives.
- Verify async hook errors/edge behavior are handled deterministically.

## Files
- `src/agent_loop/runner.rs` (tests module)

## Acceptance Criteria
- Hook invoked exactly once per provider call.
- Hook can inject/remove messages and effects are observable in provider request.
- Runs without hook are unchanged.
