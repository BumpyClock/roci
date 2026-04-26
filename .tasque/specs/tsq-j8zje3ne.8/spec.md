# Tool Argument Validation Before Execution

## Goal
Ensure tool calls are validated against each tool's JSON schema before execution, with deterministic error results and no panic paths.

## Scope
- Validate parsed model-provided arguments against `AgentToolParameters` schema.
- Return tool error result when validation fails.
- Emit matching `RunEvent`/`AgentEvent` error semantics.
- Add targeted tests for invalid/missing/extra-typed args.

## Files
- `src/agent_loop/runner.rs`
- `src/tools/arguments.rs` (if helper logic needed)
- `src/agent_loop/events.rs` (if event payload adjustments needed)

## Acceptance Criteria
- Invalid args never invoke tool handler.
- Tool result includes validation failure details.
- Valid args still execute existing tool path.
- Tests cover object shape mismatch + primitive type mismatch.
