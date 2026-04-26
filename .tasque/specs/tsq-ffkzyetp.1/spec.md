## Goal
Move runtime tests out of inline `runtime.rs` test block into dedicated test files without changing behavior.

## Why
Current inline tests contribute major file bloat and slow refactor iteration.

## Scope
- Source file: `crates/roci-core/src/agent/runtime.rs`
- Introduce test module tree (example shape):
  - `crates/roci-core/src/agent/runtime_tests/mod.rs`
  - `crates/roci-core/src/agent/runtime_tests/support.rs`
  - `crates/roci-core/src/agent/runtime_tests/state_lifecycle.rs`
  - `crates/roci-core/src/agent/runtime_tests/queue_and_continue.rs`
  - `crates/roci-core/src/agent/runtime_tests/tools_and_dynamic.rs`
  - `crates/roci-core/src/agent/runtime_tests/snapshot.rs`
  - `crates/roci-core/src/agent/runtime_tests/api_key.rs`
  - `crates/roci-core/src/agent/runtime_tests/before_agent_start.rs`
  - `crates/roci-core/src/agent/runtime_tests/compaction_and_branch_summary.rs`
  - `crates/roci-core/src/agent/runtime_tests/session_before_compact.rs`
  - `crates/roci-core/src/agent/runtime_tests/session_before_tree.rs`
  - `crates/roci-core/src/agent/runtime_tests/value_types.rs`

## Requirements
- Keep all existing test function names unchanged.
- Preserve private access model (`super::*`) so tests can still validate internals.
- Use one thin anchor in runtime file, e.g. `#[cfg(test)] #[path = "runtime_tests/mod.rs"] mod tests;`.
- Maintain baseline parity of ~71 tests.

## Acceptance
1. Baseline and post-move test list counts match for runtime test namespace.
2. Runtime test namespace passes.
3. Full `roci-core` + `agent` tests pass.

## Verification Commands
- `cargo test -p roci-core --features agent "agent::runtime::tests::" -- --list`
- `cargo test -p roci-core --features agent "agent::runtime::tests::"`
- `cargo test -p roci-core --features agent`
