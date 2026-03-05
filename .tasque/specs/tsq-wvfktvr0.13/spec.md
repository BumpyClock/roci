## Overview
Define and validate `ask_user` tool JSON schema.

Primary file:
- `crates/roci-tools/src/builtin.rs` (schema definition)

## Constraints / Non-goals
- Schema should be strict enough for safe parsing.
- Keep extensible for future question types.

## Interfaces (CLI/API)
- Request fields include question list with optional short options list.
- Response shape documented for consumer tools/models (`answers[]`, canceled flag where relevant).

## Data model / schema changes
- JSON Schema for tool parameters with bounds:
  - non-empty questions
  - option count limits
  - required ids/text fields

## Acceptance criteria
- Invalid args fail validation before execution.
- Valid request args accepted by tool framework.

## Test plan
- Add schema validation tests in `roci-tools` (`.18` expands coverage).
