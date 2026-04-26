## Goal
Extract remaining runtime lifecycle, state, and mutation APIs into focused modules and leave `runtime.rs` as a thin orchestrating facade.

## Scope
- Create focused files (or equivalent split) for:
  - lifecycle methods (`prompt`, `continue*`, `steer`, `follow_up`, `abort`, `reset`, `wait_for_idle`)
  - state/snapshot helpers (`state`, `watch_state`, `snapshot`, state guards, broadcasts)
  - idle-only mutators (`set_model`, `set_system_prompt`, `replace_messages`, `set_tools`, queue clear methods)
- Keep `AgentRuntime` struct location stable and preserve field semantics.

## Constraints
- Preserve lock/try_lock behavior and error strings where possible.
- Maintain method visibility exactly (public vs private).
- Avoid introducing deadlocks from changed lock order.

## Acceptance
1. All lifecycle and snapshot tests pass.
2. `continue_without_input` guard semantics unchanged.
3. Runtime file is significantly reduced; remaining extracted files are reasonably bounded.

## Verification Commands
- `cargo test -p roci-core --features agent "agent::runtime::tests::state_lifecycle::"`
- `cargo test -p roci-core --features agent "agent::runtime::tests::queue_and_continue::"`
- `cargo test -p roci-core --features agent "agent::runtime::tests::snapshot::"`
