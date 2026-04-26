## Overview
Add a stdio host adapter for the shared Roci MCP server core.

## Scope
- Wire the server core to stdio transport hosting.
- Ensure clean startup/shutdown and deterministic close semantics.

## Acceptance criteria
1. Stdio host can serve `tools/list` and `tools/call` using the shared server core.
2. Host lifecycle tests cover start, normal shutdown, and error-path shutdown.
