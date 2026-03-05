## Overview
Add core runner tests for user-input flow.

Primary files:
- `crates/roci-core/src/agent_loop/runner/tests/*`
- `crates/roci-core/src/agent/runtime_tests/*`

## Constraints / Non-goals
- Tests must be deterministic (no real stdin).
- Prefer mocked handler/channels.

## Interfaces (CLI/API)
- Cover runner/runtime request-response APIs and emitted events.

## Data model / schema changes
- None.

## Acceptance criteria
- Tests cover:
  - request event emitted
  - response unblocks
  - timeout path
  - unknown request id error

## Test plan
- `cargo test -p roci-core --features agent`
- Include new test names in task note/spec once added.
