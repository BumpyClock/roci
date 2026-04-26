## Overview
Add first-class remote MCP client transports for streamable HTTP and WebSocket while preserving stdio behavior.

## Scope
- Introduce `StreamableHttpTransport` as the canonical remote HTTP client transport.
- Replace the current `SSETransport` public surface instead of keeping a compatibility alias by default.
- Add `WebSocketTransport` for MCP JSON-RPC over WebSocket.
- Define shared remote transport config and error mapping conventions.

## Non-goals
- No auth UX in this task.
- No app-level reconnect state machine in this task.
- No server-side hosting.

## API / contract
- Public constructors/config must support:
  - URL
  - static headers
  - optional auth hook/token injection point
  - request/connect timeout knobs
- `MCPClient` must be able to initialize through either transport without app-specific glue.
- Transport errors must map into deterministic `RociError` variants.

## Acceptance criteria
1. Streamable HTTP and WebSocket transports can initialize an MCP session and execute `tools/list` + `tools/call`.
2. Current stdio transport remains unchanged for existing callers.
3. The old remote-transport naming is removed or kept only as an internal migration detail; the public API presented by this task is `StreamableHttpTransport` + `WebSocketTransport`.
4. Integration tests cover happy path, timeout/close behavior, and malformed/closed-peer failures for both remote transports.

## Validation
- Targeted transport tests under `cargo test -p roci-core --features mcp`.
- Verify no regression in existing stdio + aggregation tests.
