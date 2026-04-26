## Overview
Add an HTTP host adapter for the shared Roci MCP server core.

## Scope
- Define the HTTP hosting boundary for MCP server mode.
- Reuse shared server-core request dispatch and serialization.
- Keep bind/listen/auth concerns configurable by the outer host application.

## Acceptance criteria
1. HTTP host can serve MCP requests with the shared server core.
2. Host-specific concerns (bind address, middleware, outer auth hooks) are explicit and do not leak into tool mapping.
3. Tests cover happy path and host-level error handling.
