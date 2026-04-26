## Overview
Build the transport-agnostic MCP server core that maps Roci tools/providers onto MCP server operations.

## Scope
- Map `Tool` / `DynamicToolProvider` metadata into MCP tool schemas.
- Implement shared request dispatch for `tools/list` and `tools/call`.
- Define server-core error/result mapping once for all host adapters.

## Acceptance criteria
1. One server-core type can be reused by stdio, HTTP, and WebSocket hosts.
2. Tool schema generation and tool-call dispatch are not duplicated in host adapters.
3. Tests cover tool listing, successful execution, and MCP error-result mapping.
