# Steering Message Handling

## Goal
Validate steering behavior added by loop core: queued steering messages preempt remaining tool work and are injected before the next model turn.

## Scope
- Add runner tests for steering during:
  - parallel-safe tool batch execution,
  - sequential tool execution.
- Verify skipped tool calls emit deterministic skipped results.
- Verify steering messages are appended before next provider request.

## Files
- `src/agent_loop/runner.rs` (tests module)

## Acceptance Criteria
- Steering callback returning messages causes loop to stop processing remaining pending tool calls.
- Skipped calls produce stable message text (`Skipped due to queued user message`).
- Next LLM request includes steering messages in-order.
- No regression for runs with no steering messages.
