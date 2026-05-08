## Overview
Add first-class remote MCP client transports for streamable HTTP and WebSocket while preserving stdio behavior.

## Scope
- Introduce `StreamableHttpTransport` as the canonical remote HTTP client transport.
- Replace the current `SSETransport` public surface instead of keeping a compatibility alias by default.
- Add `WebSocketTransport` for MCP JSON-RPC over WebSocket.
- Define shared remote transport config and error mapping conventions.
- Follow the Codex-style transport path: transport implementations own session lifecycle, headers/auth injection, timeout handling, content-type/SSE handling, and deterministic recovery errors.

## Non-goals
- No auth UX in this task.
- No app-level reconnect state machine in this task.
- No server-side hosting.
- No MCP server-core request dispatch in this task; `tsq-p4cpczyg.2.1` owns transport-agnostic server behavior.

## API / contract
- Public constructors/config must support:
  - URL
  - static headers
  - optional auth hook/token injection point
  - request/connect timeout knobs
- `MCPClient` must be able to initialize through either transport without app-specific glue.
- Transport errors must map into deterministic `RociError` variants.
- Streamable HTTP must support JSON and `text/event-stream` responses, MCP session ids, close/delete semantics where available, and auth/header injection without tying the SDK to a concrete UX.
- WebSocket must expose the same `MCPClient` contract and deterministic connect/close/malformed-peer errors.

## Interfaces (CLI/API)
```rust
pub struct StreamableHttpTransportConfig {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub auth: Option<McpAuthHook>,
    pub request_timeout_ms: Option<u64>,
}

pub struct WebSocketTransportConfig {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub auth: Option<McpAuthHook>,
    pub connect_timeout_ms: Option<u64>,
}
```

## Data model / schema changes
- Replace the public remote HTTP/SSE naming surface with `StreamableHttpTransport`.
- Add typed remote transport configs for Streamable HTTP and WebSocket.
- Add deterministic transport error mapping for timeout, auth, closed session, unsupported content type, malformed message, and closed peer.
- Preserve stdio transport config and behavior for existing callers.

## Acceptance criteria
1. Streamable HTTP and WebSocket transports can initialize an MCP session and execute `tools/list` + `tools/call`.
2. Current stdio transport remains unchanged for existing callers.
3. The old remote-transport naming is removed or kept only as an internal migration detail; the public API presented by this task is `StreamableHttpTransport` + `WebSocketTransport`.
4. Integration tests cover happy path, timeout/close behavior, and malformed/closed-peer failures for both remote transports.
5. Transport tests include Codex-inspired session/content-type/error cases while preserving Roci's API shape.

## Validation
- Targeted transport tests under `cargo test -p roci-core --features mcp`.
- Verify no regression in existing stdio + aggregation tests.

## Test plan
- Streamable HTTP initialize/list/call happy-path tests.
- Streamable HTTP JSON response, `text/event-stream` response, session id, close/delete, timeout, and unsupported content-type tests.
- WebSocket initialize/list/call happy-path tests.
- WebSocket close, malformed-peer, timeout, and auth/header tests.
- Regression tests proving existing stdio transport behavior is unchanged.
