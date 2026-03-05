## Overview
Implement `ask_user` baseline end-to-end in core + CLI demo, aligned to pi-mono behavior for blocking question/answer, and preserve migration seam for future peer bus.

Reference design: `docs/architecture/ask-user-v1-peer-bus-seam.md`.

Subtasks in scope: `tsq-wvfktvr0.7`..`tsq-wvfktvr0.18` and docs `tsq-wvfktvr0.17`.

## Constraints / Non-goals
- No child-to-child peer bus in this task.
- No session tree/forking work.
- `ask_user` is blocking only.
- Core owns capabilities; CLI only demonstrates/use-cases.

## Interfaces (CLI/API)
- Core:
  - user-input types in `roci-core`.
  - runner/runtime submit-response path by `request_id`.
  - event emission for input request.
- CLI:
  - consume request event, prompt user, call core submit API.

## Data model / schema changes
- Add typed request/question/option/answer/response model in core.
- Add `AgentEvent::UserInputRequested` payload model.
- Add `ask_user` tool schema and validated output shape.

## Acceptance criteria
- `ask_user` can request input and block until response or timeout.
- Request event emitted with deterministic payload.
- CLI can prompt and submit answer back through core.
- Timeouts/invalid IDs handled deterministically.
- Core/tool/docs tests pass for this feature.

## Test plan
- Run focused tests from subtasks (`.16`, `.18`).
- Run workspace tests for touched crates:
  - `cargo test -p roci-core --features agent`
  - `cargo test -p roci-tools`
  - `cargo test -p roci-cli`
