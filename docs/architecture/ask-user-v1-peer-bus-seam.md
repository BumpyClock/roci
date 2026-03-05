# ask_user v1 With Peer-Bus-Compatible Seams

## Status
- Scope: `tsq-wvfktvr0` + open children (`.7` to `.18`, `.17` docs)
- Target: implement blocking `ask_user` now; keep architecture reversible for peer bus later.

## Goals
- Add `ask_user` as a core capability in `roci-core`, not a CLI-only behavior.
- Keep v1 behavior simple:
  - child/tool path requests user input
  - parent runtime surfaces request
  - response unblocks waiting tool call
- Preserve migration path to peer bus without reworking tool contracts.

## Non-Goals
- No child-to-child routing in v1.
- No session tree/fork implementation in this work.
- No non-blocking `ask_user` flow.

## Rules
- `ask_user` is blocking and deterministic.
- Unknown `request_id` response returns typed error.
- Timeout returns explicit canceled/timeout result shape; never panic.
- CLI is demo shell for interaction; core owns API + state + invariants.

## Core Data Model
- Define stable user-input types in `roci-core`:
  - request envelope (`request_id`, `tool_call_id`, questions, timeout)
  - question/option/answer types
  - response envelope (`request_id`, answers, canceled flag)
- Keep fields and naming neutral so later peer messaging can reuse payloads.

## Event Model
- Add `AgentEvent::UserInputRequested` in core event stream.
- Optional `UserInputReceived` can be deferred if not needed immediately.
- Event payload must include enough data for external UIs/hosts to render prompts.

## Runtime + Runner Contract
- Add user-input callback + timeout config to runtime/run request surfaces.
- Add submission API:
  - `RunHandle::submit_user_input(...)`
  - `AgentRuntime::submit_user_input(...)`
- Add request/response wait path with correlation by `request_id`.
- Keep internals behind small interfaces so routing policy can evolve.

## Future Peer Bus Seam (Do Not Implement Yet)
- Keep one generic internal message envelope shape and status lifecycle in mind.
- Avoid hardcoding parent-only assumptions into core type names.
- Enforce v1 routing policy in logic, not in schema shape.
- Future step can swap routing policy to allow additional destinations.

## CLI Responsibilities (Demo)
- Consume `UserInputRequested`.
- Prompt human via stdin UX.
- Submit response back through core API.
- Do not duplicate core validation/state logic.

## Acceptance Criteria
- `ask_user` available through core tool execution path.
- Core emits request event and waits for response.
- Response unblocks tool execution.
- Timeout and cancellation covered with deterministic errors/results.
- Tests exist for happy path + timeout + unknown request id + schema validation.
- Docs updated in architecture + built-in tools references.

## Test Matrix
- Runner tests:
  - event emitted with expected payload
  - submit response -> unblocks
  - timeout -> canceled/error outcome
  - unknown request id -> error
- Tool tests:
  - schema required fields and bounds
  - missing callback returns explicit tool error
- CLI tests (if harness supports):
  - request event surfaces prompt flow
  - user response is passed back via runtime API

## Implementation Notes
- Keep public APIs minimal and typed.
- Prefer additive changes over signature breakage where possible.
- Include migration comments where v1 choices intentionally preserve v2 path.
