## Context
`crates/roci-core/src/agent/runtime.rs` is currently ~3035 LOC (including 71 tests), violating repo guidance to keep files around <=500 LOC and making iterative changes high-risk.

Evidence gathered:
- `wc -l crates/roci-core/src/agent/runtime.rs` => 3035
- Existing overlapping tasks: `tsq-2gezsbq3`, `tsq-3d9jvsbf`, `tsq-herw41aj`
- Current task `tsq-ffkzyetp` has no spec/dependencies; needs full breakdown for agent handoff.

## Objective
Refactor runtime implementation into smaller focused modules while preserving behavior and public API of `roci::agent::runtime`.

## Non-goals
- No behavioral redesign of agent runtime flow.
- No API contract changes for external consumers.
- No provider/tool protocol changes beyond mechanical extraction.

## Public API Must Stay Stable
At minimum keep these exported symbols and semantics intact:
- `AgentConfig`
- `AgentRuntime`
- `AgentSnapshot`
- `AgentState`
- `QueueDrainMode`
- `GetApiKeyFn`
- `SessionBeforeCompactHook`
- `SessionBeforeCompactPayload`
- `SessionBeforeTreeHook`
- `SessionBeforeTreePayload`
- `SessionSummaryHookOutcome`
- `SummaryPreparationData`

## Target Module Shape (guideline)
- `crates/roci-core/src/agent/runtime.rs` (facade + re-exports; minimal glue)
- `crates/roci-core/src/agent/runtime/types.rs`
- `crates/roci-core/src/agent/runtime/config.rs`
- `crates/roci-core/src/agent/runtime/summary.rs`
- `crates/roci-core/src/agent/runtime/run_loop.rs`
- `crates/roci-core/src/agent/runtime/lifecycle.rs`
- `crates/roci-core/src/agent/runtime/state.rs`
- `crates/roci-core/src/agent/runtime/mutations.rs`
- `crates/roci-core/src/agent/runtime/events.rs`
- test modules split into dedicated files under runtime test path

## Execution Plan
1. Move tests out of giant inline block (lowest behavior risk).
2. Extract shared types + config.
3. Extract compaction and branch-summary logic.
4. Extract run-loop orchestration and event sink.
5. Extract lifecycle/state/mutator methods and reduce facade.
6. Run verification + docs sync.

## Parallelization & Dependencies
- `.1` (tests extraction) can run first and unblock safer code moves.
- `.2` should finish before `.3/.4/.5` to stabilize shared imports.
- `.3` and `.4` can run in parallel after `.2`.
- `.5` starts after `.3` and `.4`.
- `.6` depends on `.1-.5` completion.

## Global Acceptance Criteria
- Runtime behavior unchanged (all existing runtime tests retained and green).
- Public exports unchanged from consumer perspective.
- Refactored files are each around <=500 LOC target where practical.
- Commands pass:
  - `cargo test -p roci-core --features agent`
  - `cargo clippy -p roci-core --all-targets --features agent -- -D warnings`
  - `cargo fmt --all -- --check`
- If API/architecture docs become stale, update docs before closing.

## Notes For Assigned Coding Agents
- Preserve method signatures and error semantics exactly.
- Watch high-risk areas: state transitions, API key precedence, compaction override validation, snapshot event timing.
- If a helper must remain >500 LOC temporarily, note why in task completion notes and open follow-up task.
