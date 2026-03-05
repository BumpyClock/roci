## Overview
Wire user input handler + timeout configuration from AgentConfig to run request execution.

Primary files:
- `crates/roci-core/src/agent/runtime/config.rs`
- `crates/roci-core/src/agent/runtime/run_loop.rs`
- `crates/roci-core/src/agent_loop/runner.rs` request structs

## Constraints / Non-goals
- Defaults must preserve existing behavior when handler unset.
- No CLI prompt behavior here.

## Interfaces (CLI/API)
- Add config/request fields:
  - `user_input_handler`
  - `user_input_timeout_ms` (or duration equivalent)

## Data model / schema changes
- Config-only additions.

## Acceptance criteria
- AgentConfig fields are propagated into runner.
- Timeout is honored when no response arrives.

## Test plan
- Add propagation tests and default-behavior tests in runtime/runner suites.
