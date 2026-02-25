## Overview
Close MCP parity at the **library** layer (CLI is demo-only) using rmcp-based stdio + SSE transports, with multi-server aggregation and server-instruction integration.

## Constraints / Non-goals
- Non-goal: CLI UX/flags are required for closure.
- Keep MCP feature optional behind `mcp` feature flag.
- Do not regress existing non-MCP agent/runtime behavior.
- Prefer deterministic error semantics over implicit fallback.
- Locked decisions:
  - Tool collision policy: auto-namespace as `<server_id>__<tool_name>`.
  - Multi-server init policy: fail-fast strict default.
  - Instruction merge policy: append MCP instruction block after existing system prompt.
  - Reconnect/version fallback: in scope for this task.

## Interfaces (CLI/API)
- Public library API must support:
  - Constructing MCP clients from stdio and SSE transport configs.
  - Initializing without requiring pre-attached `RunningService`.
  - Aggregating multiple MCP servers as one dynamic tool provider.
  - Exposing aggregated server instructions for caller-controlled prompt composition.

## Data model / schema changes
- Add explicit server identity metadata for routing/collision handling.
- Add instruction payload shape (single-server + aggregated).
- Add deterministic tool routing identifiers for multi-server execution.

## Acceptance criteria
1. `MCPClient::new(Box<dyn MCPTransport>)` can initialize and execute `tools/list` and `tools/call` end-to-end.
2. Multi-server aggregation works across >=2 servers with deterministic collision behavior.
3. Server `instructions` are retrievable and mergeable through library helpers.
4. Core runtime/agent APIs can consume MCP tool providers without CLI-only glue.
5. Stdio and SSE paths are both covered by integration/e2e tests.
6. Docs + `feature-gap-analysis.md` reflect completion status and constraints.

## Test plan
- Unit tests: transport config, routing, collision policy, instruction merge behavior.
- Integration tests: stdio server bootstrap + tool discovery/execution.
- Integration tests: SSE server bootstrap + tool discovery/execution.
- Multi-server tests: aggregation, routing, collisions, instruction merge output.
- Regression tests: existing runner/runtime tests with and without `mcp` feature.
