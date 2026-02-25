# Epic Spec: Complete stub/partial implementations across 8 modules

## Goal
Close all known stub/partial areas with production-grade implementations and robust tests.

## Strategic Decisions
1. Use official MCP Rust SDK crate `rmcp` for MCP protocol/transport behavior.
2. Keep a thin `roci::mcp` adapter boundary so future SDK changes are isolated.
3. Execute in hardening mode: full error handling, protocol validation, and deep tests.

## Scope
- MCP transports and client behavior (`src/mcp/*`).
- MCP bridge into Roci dynamic tool system.
- Realtime websocket connection for audio.
- Concrete transcription/TTS providers.
- Auth manager file persistence.
- Test expansion for auth/stream_transform/config/error/util modules.

## Out of Scope
- New UI/TUI surfaces.
- Broad API redesign unrelated to stubs/partial gaps.

## Quality Bar
- No remaining `UnsupportedOperation` in scoped tasks.
- Unit + integration tests for success and failure paths.
- Deterministic behavior for timeout/cancellation/retry handling.
- Documentation updates where API behavior changed.

## Definition of Done
- Child tasks complete with tests passing (`cargo test`).
- No regression in existing tests.
- Task specs attached and planning state moved to `planned`.
