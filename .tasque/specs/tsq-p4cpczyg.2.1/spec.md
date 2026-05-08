## Overview
Build the transport-agnostic MCP server core that maps Roci tools/providers onto MCP server operations.

## Scope
- Map `Tool` / `DynamicToolProvider` metadata into MCP tool schemas.
- Implement shared request dispatch for `tools/list` and `tools/call`.
- Define server-core error/result mapping once for all host adapters.
- Keep server core transport-agnostic; stdio, Streamable HTTP, and WebSocket hosts call the same core.
- Preserve raw Roci tool identity separately from exposed MCP names so host adapters never parse `mcp__<server_id>__<tool_name>` for routing.

## Constraints / Non-goals
- No stdio/HTTP/WebSocket listener implementation in server core.
- No auth UX or host security boundary.
- No upstream MCP proxying in v1.
- No duplicated tool schema generation in host adapters.

## Interfaces (CLI/API)
```rust
pub enum McpServerToolIdentity {
    Native { name: String },
    Mcp { server_id: String, tool_name: String },
}

pub struct McpServerCore {
    // opaque core state
}

impl McpServerCore {
    pub async fn list_tools(&self) -> Result<Vec<McpToolSchema>, RociError>;
    pub async fn call_tool(
        &self,
        identity: McpServerToolIdentity,
        arguments: serde_json::Value,
    ) -> Result<McpCallToolResult, RociError>;
}
```

## Architecture decision
- Follow the Codex path for separation of concerns: protocol/transport lifecycle stays outside the server core, and the server core owns MCP method behavior and error/result mapping.
- Pi's wrapper-pattern lesson applies to schema adaptation only; Pi does not define the MCP protocol shape for this task.

## Native vs aggregated tool identity
Native Roci tools exposed by MCP server mode keep their native/plain tool names. The `mcp__<server_id>__<tool_name>` contract applies to aggregated downstream MCP tools, not native Roci server exports. If future server mode exposes aggregated MCP tools, server core must carry `ToolIdentity::{Native, Mcp}` and route by that structured identity.

## Data model / schema changes
- Add transport-agnostic MCP tool schema mapping for Roci `Tool` and `DynamicToolProvider`.
- Add server-core result/error mapping from Roci tool outcomes to MCP `tools/call` results.
- Add structured tool identity for native and future aggregated MCP tool exports.

## Acceptance criteria
1. One server-core type can be reused by stdio, HTTP, and WebSocket hosts.
2. Tool schema generation and tool-call dispatch are not duplicated in host adapters.
3. Tests cover tool listing, successful execution, and MCP error-result mapping.
4. Tests prove the server core routes by structured tool identity, not by reparsing exposed names.

## Test plan
- Unit tests for Roci tool metadata -> MCP schema conversion.
- Unit tests for `tools/list` ordering and schema stability.
- Unit tests for successful `tools/call` dispatch.
- Error mapping tests for validation failure, unknown tool, tool runtime error, and canceled execution.
- Identity tests proving native tools route by native identity and future aggregated tools route by structured MCP identity.
