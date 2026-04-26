# Task Spec: MCP stdio transport via rmcp

## Deliverables
- Replace `StdioTransport` stub with rmcp child-process transport wrapper.
- Process lifecycle: spawn, readiness, shutdown, kill-on-drop/cancel.
- Error mapping to `RociError` for spawn/io/protocol failures.

## Acceptance Criteria
- Valid JSON-RPC exchange works against fixture MCP stdio server.
- Cancellation/close releases process resources.
- Broken pipe/process exit surfaces deterministic errors.

## Tests
- Integration: happy path round trip.
- Integration: server exits unexpectedly.
- Integration: timeout/cancel path.
