## Goal
Extract run-loop orchestration and event interception logic into dedicated modules, preserving runtime lifecycle semantics.

## Scope
- Create:
  - `crates/roci-core/src/agent/runtime/run_loop.rs`
  - `crates/roci-core/src/agent/runtime/events.rs`
- Move methods:
  - `resolve_tools_for_run`
  - `merge_static_and_dynamic_tools`
  - `run_loop`
  - `build_intercepting_sink`
- Keep runtime facade delegating to these modules.

## Behavior Guards
- Preserve `AgentState` transitions: Idle -> Running -> Idle.
- Preserve `last_error` and `is_streaming` updates across success/failure.
- Preserve API-key precedence:
  1) `request.api_key_override`
  2) config/provider key
  3) `get_api_key` callback
- Preserve dynamic tool merge behavior and order.
- Preserve event forwarding + snapshot update timing.

## Acceptance
1. Lifecycle, tool merge, and hook tests pass.
2. No regressions in abort/wait semantics.
3. Event sink still forwards all events to caller-provided sink.

## Verification Commands
- `cargo test -p roci-core --features agent "agent::runtime::tests::state_lifecycle::"`
- `cargo test -p roci-core --features agent "agent::runtime::tests::tools_and_dynamic::"`
- `cargo test -p roci-core --features agent "agent::runtime::tests::before_agent_start::"`
- `cargo test -p roci-core --features agent "agent::runtime::tests::api_key::"`
