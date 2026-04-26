## Overview
Run final verification and produce closure notes once remediation tasks complete.

## Constraints / Non-goals
- No new feature work.
- Do not close parent task until verification is green or residual issues are documented.

## Interfaces (CLI/API)
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features`
- `cargo test`

## Data model / schema changes
None.

## Acceptance criteria
- Formatting, linting, and tests run successfully or failures are documented with explicit follow-up tasks.
- Task history and notes include summary of fixed vs deferred warnings.
- Parent feature is ready for closure decision.

## Test plan
- Execute full validation command set in clean working tree state.
- Spot-check changed files for unintended edits.
