## Overview
Apply straightforward, low-risk clippy fixes in `roci-core` that do not require architectural redesign.

## Constraints / Non-goals
- Do not tackle deep API redesign warnings here.
- Preserve semantics and existing public behavior.

## Interfaces (CLI/API)
- Candidate lints: `map_flatten`, `useless_conversion`, `derivable_impls`, `field_reassign_with_default`
- Commands: `cargo clippy -p roci-core --all-targets`, `cargo test -p roci-core`

## Data model / schema changes
None expected.

## Acceptance criteria
- Mechanical warnings in `roci-core` are reduced or eliminated.
- No regressions in `roci-core` tests.
- Any intentionally deferred warning is documented with rationale.

## Test plan
- Run crate-scoped clippy for fast feedback.
- Run crate-scoped tests before/after changes.
