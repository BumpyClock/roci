## Overview
Harden MCP server mode with cross-host integration tests and regression coverage.

## Scope
- Add integration coverage for stdio, HTTP, and WebSocket hosts.
- Validate that all hosts exercise the same server-core behavior.
- Cover failure mapping, shutdown behavior, and non-MCP regression risk.

## Acceptance criteria
1. Integration tests prove the same exported tool set behaves consistently across stdio, HTTP, and WebSocket hosts.
2. Failure mapping and shutdown behavior are covered across all three hosts.
3. Regression tests prove existing non-MCP tool execution paths remain unchanged.
