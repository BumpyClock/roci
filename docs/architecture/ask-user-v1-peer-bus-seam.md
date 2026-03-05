# ask_user v1 With Peer-Bus-Compatible Seams

## Status
- **IMPLEMENTED** (2026-03-05)
- Scope: `tsq-wvfktvr0` + children (`.7` to `.18`)
- All phases complete: canonical types, events, config, context extension, coordinator, tool execution, CLI wiring, tests, docs.

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

## Implementation Summary

### Phase 1: Canonical Types (`roci-core/src/tools/user_input.rs`)
- `UserInputRequestId` (UUID)
- `UserInputRequest` (request_id, tool_call_id, questions, timeout_ms)
- `Question` (id, text, options)
- `QuestionOption` (id, label)
- `UserInputResponse` (request_id, answers, canceled)
- `Answer` (question_id, content)
- `UnknownUserInputRequest` (typed error)
- `UserInputError` enum (UnknownRequest, Timeout, Canceled, NoCallback)
- `RequestUserInputFn` callback type

### Phase 2: Events & Config
- `AgentEvent::UserInputRequested { request: UserInputRequest }` in `agent_loop/events.rs`
- `AgentConfig.user_input_timeout_ms: Option<u64>` in `agent/runtime/config.rs`
- `ToolExecutionContext.request_user_input: Option<RequestUserInputFn>` (agent feature only)

### Phase 3: Coordinator (`roci-core/src/agent/runtime/user_input.rs`)
- `UserInputCoordinator` manages pending requests via oneshot channels
- `create_request()` → returns receiver for tool to await
- `submit_response()` → unblocks waiting tool
- `cancel_all()` → cleanup on abort/reset
- timed-out/canceled waits remove pending entries before returning, so late submits deterministically fail with `UnknownUserInputRequest`
- completion notifications are broadcast to hosts that need to stop waiting on input
- `wait_for_response()` → helper with optional timeout

### Phase 4: Tool Execution (`roci-tools/src/builtin/ask_user.rs`)
- Parses questions from tool arguments
- Creates `UserInputRequest` with UUID
- Calls `ctx.request_user_input` callback (agent feature only)
- Returns structured response or error

### Phase 5: Runtime & CLI Integration
- `AgentRuntime.submit_user_input()` → delegates to coordinator
- `AgentRuntime.user_input_coordinator` field (agent feature only)
- Callback built in `run_loop.rs`: creates request via coordinator, emits `UserInputRequested` event, waits for response with timeout
- Callback threaded through `RunRequest.user_input_callback` → `execute_tool_call` → `ToolExecutionContext`
- CLI uses shared `UserInputCoordinator` passed via `AgentConfig.user_input_coordinator`
- CLI event sink forwards `UserInputRequested` into a dedicated prompt host
- Prompt host uses a single worker thread plus cancellable raw-mode terminal polling to avoid detached per-request stdin readers
- Prompt host checks coordinator completion/shutdown between polls so timed-out requests stop prompting without spawning orphan readers
- CLI requires an interactive terminal for `ask_user`; non-interactive/raw-mode-unavailable sessions fail fast with a typed error
- CLI `prompt_user_for_input()` collects answers and submits via coordinator
- `ask_user_tool()` added to `all_tools()`

### Key Design Decisions
1. **Feature-gated callback**: The `request_user_input` callback is only available when the `agent` feature is enabled, ensuring core remains usable without async runtime.
2. **Blocking semantics**: The tool blocks until response is received, timeout expires, or request is canceled.
3. **Deterministic errors**: All error cases return typed errors, never panic.
4. **Peer-bus ready**: Type names avoid parent-specific terminology; routing policy can be swapped later.
5. **Shared coordinator**: The CLI and runtime share a `UserInputCoordinator` via `AgentConfig`, enabling the event sink to submit responses directly without going through `AgentRuntime::submit_user_input()`.
