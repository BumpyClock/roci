# Feature Flag: roci-agent

## Goal
Gate new runtime behind `roci-agent` Cargo feature for controlled rollout.

## Scope
- Add feature wiring in `Cargo.toml`.
- Gate modules/APIs requiring new runtime dependencies.
- Ensure default build remains stable.
- Add CI/test command updates if needed.

## Files
- `Cargo.toml`
- feature-gated module files in `src/agent/`, `src/agent_loop/`, `src/tools/`

## Acceptance Criteria
- `cargo test` passes default feature set.
- `cargo test --features roci-agent` passes.
- New dependencies only activated with feature where intended.
