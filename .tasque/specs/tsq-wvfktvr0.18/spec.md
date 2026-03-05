## Overview
Add schema-focused tests for ask_user tool parameters and results.

Primary files:
- `crates/roci-tools/src/builtin.rs`
- `crates/roci-tools/tests/*` or module tests

## Constraints / Non-goals
- Focus on schema bounds/validation, not runtime channel behavior.

## Interfaces (CLI/API)
- Validate tool argument schema contract exposed to model providers.

## Data model / schema changes
- Assert required/optional behavior and min/max constraints.

## Acceptance criteria
- Invalid payloads rejected with deterministic validation errors.
- Valid payloads accepted.

## Test plan
- `cargo test -p roci-tools`
- Explicit cases: missing fields, empty questions, invalid options shape, oversized option counts.
