## Overview
Wire MCP providers into core agent/runtime API (library path).

## Constraints / Non-goals
- Non-goal: CLI-specific registration flow.
- Must preserve existing static tool registration behavior.

## Interfaces (CLI/API)
- Runtime/agent config supports dynamic MCP provider(s).
- Dynamic tools participate in normal tool-call execution loop.

## Data model / schema changes
- Agent/runtime config additions for dynamic providers and optional instruction merge integration.

## Acceptance criteria
1. Library consumer can pass MCP provider(s) to runtime without CLI glue.
2. Agent loop can execute dynamic MCP tools end-to-end.
3. Existing non-MCP tool path remains unchanged.

## Test plan
- Runtime integration test with dynamic provider-backed tools.
- Regression tests for built-in tool-only flows.
