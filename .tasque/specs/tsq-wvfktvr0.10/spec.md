## Overview
Implement runner/runtime response channel for pending user-input requests.

Primary files (expected):
- `crates/roci-core/src/agent_loop/runner.rs` and/or runner internals
- `crates/roci-core/src/agent/runtime.rs` + runtime modules

## Constraints / Non-goals
- Blocking request/response only.
- Unknown `request_id` must error cleanly.
- Keep internals policy-driven; avoid hardcoding peer-bus-incompatible names.

## Interfaces (CLI/API)
- Add core APIs:
  - `RunHandle::submit_user_input(...)`
  - `AgentRuntime::submit_user_input(...)`
- Maintain per-run pending request mapping keyed by `request_id`.

## Data model / schema changes
- Correlation by request id.
- Track pending state until response, timeout, or cancellation.

## Acceptance criteria
- submit for valid `request_id` unblocks waiter.
- submit for unknown `request_id` returns typed error.
- cancellation/timeout terminal states are deterministic.

## Test plan
- Add runner/runtime tests for submit success + unknown id + timeout path.
