# Task Spec: MCPClient methods via rmcp

## Deliverables
- Implement `initialize` with capability negotiation.
- Implement `list_tools` and MCP schema conversion.
- Implement `call_tool` with argument/result/error handling.

## Acceptance Criteria
- Client handshake validates server response shape.
- `list_tools` returns normalized `MCPToolSchema` entries.
- `call_tool` returns structured success/error values.

## Tests
- Unit/integration for initialize/list/call success.
- Malformed payload and protocol violation tests.
- Timeout/cancellation tests.
