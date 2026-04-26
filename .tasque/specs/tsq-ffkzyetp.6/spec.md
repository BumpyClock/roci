## Goal
Finalize runtime split with full verification, lint/format, and docs synchronization.

## Scope
- Run full validation after `.1-.5` land.
- Update architecture/contributing docs only if module paths or guidance changed.
- Record any remaining >500 LOC hotspots as follow-up tasks.

## Acceptance Checklist
- `cargo fmt --all -- --check`
- `cargo clippy -p roci-core --all-targets --features agent -- -D warnings`
- `cargo test -p roci-core --features agent`
- If docs are stale, update:
  - `docs/ARCHITECTURE.md`
  - `docs/contributing.md` (if file-size guidance examples/reference paths changed)
- Confirm no net loss of runtime test coverage.

## Deliverable
Task notes must include:
- final module/file layout
- commands run and outcomes
- any intentional deviations from <=500 LOC target
- any discovered follow-up task IDs
