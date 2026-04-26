## Overview
Apply straightforward clippy fixes in `roci-providers` and example binaries.

## Constraints / Non-goals
- Keep provider behavior unchanged.
- Defer architecture-heavy issues to design-level task.

## Interfaces (CLI/API)
- Candidate lints: `collapsible_if`, `needless_borrow`, `single_match`
- Commands: `cargo clippy -p roci-providers --all-targets`, `cargo clippy --examples`

## Data model / schema changes
None expected.

## Acceptance criteria
- Mechanical warnings in providers/examples are reduced.
- Example programs still compile successfully.
- Deferred warnings are explicitly tracked.

## Test plan
- Run clippy for providers and examples.
- Run relevant integration/unit tests if touched code paths require it.
