## Overview
Implement `ask_user` tool execution logic using context callback.

Primary file:
- `crates/roci-tools/src/builtin.rs` (ask_user tool implementation)

## Constraints / Non-goals
- If callback missing, return explicit deterministic tool error.
- Do not embed CLI-specific prompting logic.

## Interfaces (CLI/API)
- `ask_user` execute path calls context user-input callback.
- Returns structured result with answers/canceled metadata.

## Data model / schema changes
- Uses types from core (`.7`) and schema from `.13`.

## Acceptance criteria
- Tool blocks until callback response or timeout/cancel.
- Missing callback path returns structured error result.

## Test plan
- Add tool tests with mocked callback for success and missing-callback failure.
