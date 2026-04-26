## Overview
Add a WebSocket host adapter for the shared Roci MCP server core.

## Scope
- Define the WebSocket hosting boundary for MCP server mode.
- Reuse shared server-core request dispatch and serialization.
- Handle connection lifecycle, close semantics, and per-connection state without duplicating tool logic.

## Acceptance criteria
1. WebSocket host can serve MCP requests with the shared server core.
2. Connection lifecycle and shutdown behavior are deterministic.
3. Tests cover happy path, disconnects, and malformed/closed-peer behavior.
