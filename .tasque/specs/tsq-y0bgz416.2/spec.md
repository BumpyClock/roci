## Overview
Implement rmcp session bootstrap from transport in `MCPClient`.

## Constraints / Non-goals
- Must work for both `StdioTransport` and `SSETransport`.
- Non-goal: multi-server behavior.
- Reconnect and MCP protocol version fallback behavior are in scope.

## Interfaces (CLI/API)
- `MCPClient::initialize()` should establish a session from configured `MCPTransport` when none is attached.
- Preserve support for externally attached `RunningService`.

## Data model / schema changes
- Internal client state transitions updated for bootstrap lifecycle and reconnect-safe close behavior.

## Acceptance criteria
1. `initialize()` no longer errors when transport is present and session absent.
2. `list_tools()` and `call_tool()` succeed after transport-backed init.
3. Error mapping remains deterministic for transport/init failures.
4. Existing explicit `from_running_service` path still works.
5. Reconnect path and protocol version fallback are covered by tests.

## Test plan
- Unit tests for init from transport and init-from-attached-session.
- Negative tests for closed/failed transport and malformed init responses.
