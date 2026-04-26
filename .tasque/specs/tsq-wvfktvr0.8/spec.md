## Overview
Add `AgentEvent` support for user input request lifecycle.

Primary file:
- `crates/roci-core/src/agent_loop/events.rs`

## Constraints / Non-goals
- Event should expose renderable info; do not include CLI formatting.
- Optional `UserInputReceived` can be deferred unless needed by runner wiring.

## Interfaces (CLI/API)
- Add `AgentEvent::UserInputRequested { ... }` payload with:
  - `request_id`
  - `tool_call_id`
  - `questions`
  - `timeout_ms`

## Data model / schema changes
- Payload uses core user-input question types from `.7`.

## Acceptance criteria
- Event variant available in public event enum.
- Event payload stable and serializable.
- Existing event consumers compile after updates.

## Test plan
- Add/update event serialization tests and any match exhaustiveness tests.
