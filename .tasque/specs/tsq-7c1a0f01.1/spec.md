## Overview
Capture a fresh, complete inventory of clippy findings and group them by crate, file, lint code, and fix type.

## Constraints / Non-goals
- No code changes in this task except optional instrumentation to collect lint output.
- Avoid subjective prioritization without documented rationale.

## Interfaces (CLI/API)
- `cargo clippy --all-targets --all-features`
- Optional machine output: `cargo clippy --all-targets --all-features --message-format=json`

## Data model / schema changes
No code schema changes. Output stored as task notes or summary artifacts only.

## Acceptance criteria
- Findings matrix exists with severity buckets: compile error, mechanical warning, design/policy warning.
- Each finding is mapped to an owning child task or noted as out-of-scope.
- Unknown/ambiguous warnings are explicitly flagged.

## Test plan
- Re-run clippy once to confirm findings are reproducible.
- Compare findings list against child task scopes for coverage gaps.
