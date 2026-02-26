# Learned Index

- [Agent Message Conversion Path](./agent-message-conversion.md)
  - read_when: filtering/converting non-LLM agent messages before provider sanitize.
- [Agent Event Contracts: Message Lifecycle + Tool Updates](./agent-events-tool-updates.md)
  - read_when: wiring `AgentEvent` streaming boundaries, tool updates, cancel/fail semantics.
- [OpenAI/Gemini API Shapes](./openai-gemini-api-shapes.md)
- [OpenAI Responses Options](./openai-responses-options.md)
- [Ralph Loop Parallel](./ralph-loop-parallel.md)
- [MCP Protocol + Integration Notes](./mcp.md)
  - read_when: validate MCP transport parity (stdio/SSE/multi-server) and OpenAI instruction merge behavior.

## Architecture Decisions
- `read_when`: modifying crate boundaries, adding new crates, or changing public API surface
- See: `docs/architecture/cli-soc.md` -- CLI/core separation of concerns
