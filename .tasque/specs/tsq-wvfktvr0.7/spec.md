## Overview
Define canonical user-input types for ask_user flow in core.

Primary files:
- `crates/roci-core/src/tools/user_input.rs` (new or update)
- `crates/roci-core/src/tools/mod.rs` (re-export)

## Constraints / Non-goals
- Keep type names generic enough for future peer-bus reuse.
- No routing policy/bus implementation here.
- No CLI logic here.

## Interfaces (CLI/API)
- Public core structs/enums for:
  - request envelope (`request_id`, `tool_call_id`, `questions`, timeout)
  - question option/answer types
  - response envelope (`request_id`, `answers`, canceled/metadata)

## Data model / schema changes
- Add serde-friendly data model with explicit optional fields.
- Include validation helpers where needed (bounds/required checks).

## Acceptance criteria
- Types compile and are publicly usable from core.
- Serialization round-trip works for request and response envelopes.
- Names/fields support future non-parent-only routing.

## Test plan
- Add unit tests in `roci-core` for serialize/deserialize + validation helpers.
