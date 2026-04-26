## Overview
Implement CLI prompt UX for `ask_user` request payload.

Primary file:
- `crates/roci-cli/src/chat.rs`

## Constraints / Non-goals
- Keep CLI as demo host; core remains source of truth.
- Avoid adding complex TUI work.

## Interfaces (CLI/API)
- Render one or multiple questions.
- Show options where present, plus free-form handling policy.
- Return canceled behavior consistently on EOF/empty according to finalized policy.

## Data model / schema changes
- No core schema change; consumes event/request payload.

## Acceptance criteria
- CLI prompts are readable and deterministic.
- Collected answers map back to question ids.

## Test plan
- Add unit tests for prompt parsing/answer collection where harness allows.
