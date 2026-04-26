## Goal
Extract foundational runtime shared types + `AgentConfig` out of `runtime.rs` to reduce coupling and prepare safe parallel refactors.

## Scope
- Create:
  - `crates/roci-core/src/agent/runtime/types.rs`
  - `crates/roci-core/src/agent/runtime/config.rs`
- Move into `types.rs`:
  - `AgentState`, `QueueDrainMode`, `AgentSnapshot`
  - `GetApiKeyFn`
  - `SessionBeforeCompactOutcome`, `SessionBeforeTreeOutcome`, `SessionCompactionOverride`
  - `SummaryPreparationData` (+ constructor)
  - session hook payload structs + hook type aliases
  - `drain_queue`
- Move into `config.rs`:
  - `AgentConfig` struct and docs
- Keep re-exports from `agent::runtime` unchanged.

## Constraints
- No semantic changes to field defaults or hook types.
- No mutation of `AgentRuntime` public method signatures.
- Import paths should avoid cyclic module dependencies.

## Acceptance
1. `agent::runtime` exports remain source-compatible.
2. Build passes without dead import drift.
3. Downstream modules (`run_loop`, `summary`, lifecycle) compile against extracted types.

## Verification Commands
- `cargo test -p roci-core --features agent "agent::runtime::tests::value_types::"`
- `cargo test -p roci-core --features agent`
