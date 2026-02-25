# Agent Event Contracts: Message Lifecycle + Tool Updates

read_when:
- You are wiring a UI/SDK sink to `AgentEvent` and need deterministic streaming boundaries.
- You are implementing or debugging tool streaming via `Tool::execute_ext` callbacks.
- You need expected behavior for cancel/fail/retry branches.

## Message lifecycle contract

The runner emits message lifecycle events with full `ModelMessage` payloads:

1. `MessageStart { message }`
2. `MessageUpdate { message, assistant_message_event }` for each assistant text/reasoning/tool-call delta
3. `MessageEnd { message }`

Details:
- `MessageStart`/`MessageEnd` are emitted for prompt/context messages and tool result messages too (not only assistant streaming).
- During assistant streaming, `assistant_message_event` carries the raw delta (`TextStreamDelta`) and `message` carries the latest partial assistant message snapshot.
- `MessageStart` emits once per streamed assistant message, on first text/reasoning/tool-call delta.
- `MessageEnd` emits exactly once if an assistant stream message was started.
- `MessageEnd` is emitted on:
  - normal `done`
  - stream error
  - idle timeout
  - run cancel during stream
  - stream EOF fallback path (no explicit `done`)

## Tool execution update contract

When a tool call is approved/executed, runner emits:

1. `ToolExecutionStart { tool_call_id, tool_name, args }`
2. zero or more `ToolExecutionUpdate { args, partial_result }`
3. `ToolExecutionEnd { result, is_error }`

Details:
- Updates are forwarded from `Tool::execute_ext(..., on_update)`.
- Tools that only implement `execute` remain compatible (no updates, still start/end).
- `ToolExecutionEnd.is_error` mirrors final `AgentToolResult.is_error`.
- Steering-skip paths emit `ToolExecutionStart`/`ToolExecutionEnd` and `MessageStart`/`MessageEnd` for each skipped tool call.

## Cancellation/failure behavior

- Stream failure after partial text emits `MessageEnd` before terminal `AgentEnd`.
- Cancel during in-flight tool execution emits `ToolExecutionEnd` with an error result (`{"error":"canceled"}`) before run termination.
- Run-level retries (rate limit with retry hint) do not emit message/tool end events unless a message/tool was actually started.

## Regression coverage

Runner tests cover:
- message lifecycle ordering for normal text turns
- message end emission on stream error terminal path
- tool start/update/end ordering from a stub `execute_ext` tool
- cancel during tool execution producing error end event
