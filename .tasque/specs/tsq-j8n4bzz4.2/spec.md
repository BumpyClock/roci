# Task Spec: MCP streamable HTTP/SSE transport via rmcp

## Deliverables
- Implement remote transport using rmcp streamable HTTP client.
- Support auth/custom headers and configurable timeout/retry knobs.
- Implement clean close behavior.

## Acceptance Criteria
- Connects to compliant MCP HTTP endpoint and exchanges messages.
- Retry/timeout behavior observable and deterministic.
- Close releases network resources.

## Tests
- Integration: happy path with mock MCP HTTP server.
- Integration: timeout and 5xx/retry behavior.
- Integration: malformed server response handling.
