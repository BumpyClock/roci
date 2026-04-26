# Goal
Expose existing roci-core MCP capabilities in demo CLI/runtime defaults.

# Scope
- enable MCP feature in roci-cli dependency/features
- CLI flags/config for stdio and SSE MCP servers
- runtime wiring for MCP dynamic tool providers
- MCP instruction-source merge into system prompt assembly

# Acceptance Criteria
- CLI can register MCP servers (stdio + SSE) and use exposed tools in run.
- MCP server instructions are merged into active system prompt via existing merge helper.
- Behavior covered with integration tests and docs examples.
- Keeps SoC: capability in roci-core, CLI only demonstrates/wires.

# Non-Goals
- Full extension framework/discovery.
