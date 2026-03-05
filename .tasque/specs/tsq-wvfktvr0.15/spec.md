## Overview
Wire CLI event handling + handler callback with runtime submit API.

Primary file:
- `crates/roci-cli/src/chat.rs`

## Constraints / Non-goals
- No duplication of core validation logic.
- Must interoperate with existing hook/event printing.

## Interfaces (CLI/API)
- Subscribe to `AgentEvent::UserInputRequested`.
- Prompt user (via `.14` flow).
- Submit response through runtime/core API (`submit_user_input`).

## Data model / schema changes
- None; integration wiring.

## Acceptance criteria
- End-to-end CLI demo: model tool call -> prompt -> submit -> run continues.
- Unknown/expired request surfaces clear error to user.

## Test plan
- Add integration-style CLI tests or runner-driven CLI harness coverage.
