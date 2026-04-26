# Task Spec: MCPToolAdapter bridge

## Deliverables
- Implement `list_tools` bridging MCP tools to `DynamicTool`.
- Implement `execute_tool` forwarding args/context and mapping output.
- Preserve tool errors with actionable messages.

## Acceptance Criteria
- MCP tool schemas are exposed in Roci tool system.
- Executed tool gets correct name/args/context metadata.
- Tool errors propagate without panics.

## Tests
- Unit conversion tests for schema mapping.
- E2E adapter test using fixture MCP client.
