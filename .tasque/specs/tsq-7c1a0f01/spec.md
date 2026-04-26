## Overview
Drive clippy cleanup end-to-end for this repo, starting with compile blockers and then warning remediation while preserving behavior.

## Constraints / Non-goals
- Do not change product behavior unless explicitly required.
- Keep edits minimal and aligned with existing project patterns.
- Do not attempt broad refactors unrelated to current clippy findings.

## Interfaces (CLI/API)
- Lint command: `cargo clippy --all-targets --all-features`
- Format command: `cargo fmt --all -- --check`
- Validation command: `cargo test`

## Data model / schema changes
No runtime schema or persisted data model changes expected.

## Acceptance criteria
- Task tree has complete specs for root and children.
- Planning state is set to `planned` for implementation-ready children.
- Dependencies reflect execution order and parallelizable tracks.
- Work can proceed from triage to fixes to final verification without ambiguity.

## Test plan
- Verify task readiness using `tsq ready --lane planning` and `tsq ready --lane coding`.
- Verify all task specs pass with `tsq spec check <id>`.
- During implementation, re-run fmt/clippy/tests per child task acceptance.
