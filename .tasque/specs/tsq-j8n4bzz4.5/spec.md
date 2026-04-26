# Task Spec: RealtimeSession connect hardening

## Deliverables
- Implement websocket connect/auth for OpenAI realtime endpoint.
- Session initialization handshake and basic lifecycle state.
- Heartbeat/keepalive, reconnect strategy, graceful close.

## Acceptance Criteria
- Successful connect + session init against mock server.
- Connection interruption triggers configured reconnect behavior.
- Explicit close terminates cleanly.

## Tests
- Integration: connect/init happy path.
- Integration: disconnect/reconnect path.
- Integration: auth failure path.
