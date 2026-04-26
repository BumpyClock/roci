## Overview
Refactor sanitizer loop/control-flow warnings while preserving exact sanitization behavior.

## Constraints / Non-goals
- No behavior drift in message sanitization semantics.
- Prefer clearer iteration constructs over index mutation patterns.

## Interfaces (CLI/API)
- Target warnings: `clippy::needless_range_loop`, `clippy::mut_range_bound`
- Primary file: `crates/roci-core/src/provider/sanitize.rs`

## Data model / schema changes
None.

## Acceptance criteria
- Loop warnings are fixed or narrowly deferred with rationale.
- Output behavior remains equivalent for existing inputs.

## Test plan
- Run existing sanitize-related tests and add/adjust targeted tests if needed.
- Re-run clippy for `roci-core`.
