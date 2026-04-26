## Overview
Extend tool execution context to support user input requester callback.

Primary files:
- `crates/roci-core/src/tools/tool.rs`
- `crates/roci-core/src/agent_loop/runner/tooling.rs`

## Constraints / Non-goals
- Keep callback optional for backward compatibility.
- No direct CLI dependencies.

## Interfaces (CLI/API)
- Add callback field(s) on `ToolExecutionContext` for requesting user input.
- Ensure callback shape uses core user-input request/response types.

## Data model / schema changes
- No new persisted model; context struct extension only.

## Acceptance criteria
- Existing tools continue to compile unchanged.
- `ask_user` can consume callback via `execute_ext` path.

## Test plan
- Add unit/integration coverage proving callback is injected in `execute_tool_call`.
