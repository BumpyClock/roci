## Overview
Build the transport-agnostic MCP server core that maps Roci tools/providers onto MCP server operations.

## Scope
- Map `Tool` / `DynamicToolProvider` metadata into MCP tool schemas.
- Implement shared request dispatch for `tools/list` and `tools/call`.
- Define server-core error/result mapping once for all host adapters.
- Keep server core transport-agnostic; stdio, Streamable HTTP, and WebSocket hosts call the same core.
- Preserve raw Roci tool identity separately from exposed MCP names so host adapters never parse `mcp__<server_id>__<tool_name>` for routing.

## Architecture decision
- Follow the Codex path for separation of concerns: protocol/transport lifecycle stays outside the server core, and the server core owns MCP method behavior and error/result mapping.
- Pi's wrapper-pattern lesson applies to schema adaptation only; Pi does not define the MCP protocol shape for this task.

## Native vs aggregated tool identity
Native Roci tools exposed by MCP server mode keep their native/plain tool names. The `mcp__<server_id>__<tool_name>` contract applies to aggregated downstream MCP tools, not native Roci server exports. If future server mode exposes aggregated MCP tools, server core must carry `ToolIdentity::{Native, Mcp}` and route by that structured identity.

## Acceptance criteria
1. One server-core type can be reused by stdio, HTTP, and WebSocket hosts.
2. Tool schema generation and tool-call dispatch are not duplicated in host adapters.
3. Tests cover tool listing, successful execution, and MCP error-result mapping.
4. Tests prove the server core routes by structured tool identity, not by reparsing exposed names.
