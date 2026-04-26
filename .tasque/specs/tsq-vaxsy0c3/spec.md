# Refactor agent_loop/runner.rs into modules

## Scope
- Split `src/agent_loop/runner.rs` into smaller modules (~500 LOC or less per file).
- Preserve public API and behavior; no semantic changes.
- Update module imports and tests accordingly.

## Acceptance criteria
1) `runner.rs` reduced to orchestration + module glue.
2) Tests continue to pass for agent loop.
3) No behavioral changes beyond refactor.
