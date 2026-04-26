## Overview
Implement MCP server mode as an SDK primitive that exposes Roci tools through a shared MCP server core plus stdio, HTTP, and WebSocket hosting adapters.

## Scope
- Add a server-core adapter from Roci `Tool` / `DynamicToolProvider` surfaces to MCP `tools/list` + `tools/call`.
- Provide stdio hosting as a first-class adapter.
- Provide HTTP hosting as a first-class adapter.
- Provide WebSocket hosting as a first-class adapter.
- Keep all three hosts thin over the same tool-mapping and request-dispatch core.

## Non-goals
- No proxying of upstream MCP servers through Roci in this epic.
- No multitenant/session-distributed hosting architecture.
- No resource/prompt hosting unless a native Roci provider for them already exists.
- No opinionated production auth stack beyond clear hooks/boundaries for host integration.

## API / contract
- Callers can build an MCP server from a deterministic set of Roci tools/providers.
- Tool schemas must derive from existing Roci parameter metadata.
- Tool execution errors must map to MCP error results without panics.
- Public server-mode API should separate:
  - server core construction
  - stdio host adapter
  - HTTP host adapter
  - WebSocket host adapter
- Host adapters must not fork tool mapping or request handling logic.

## Acceptance criteria
1. A library consumer can expose a Roci tool set over stdio MCP and successfully handle `tools/list` + `tools/call`.
2. A library consumer can expose the same server core over HTTP and WebSocket MCP hosts without redefining tool mapping.
3. Dynamic tool providers are either supported directly or explicitly excluded with a documented rationale before coding starts.
4. Tool naming and descriptions are stable and consistent with the namespacing contract.
5. Tests cover tool listing, successful execution, and failure/error-result mapping across stdio, HTTP, and WebSocket hosts.

## Validation
- Integration tests against MCP client fixtures using stdio, HTTP, and WebSocket.
- Regression test proving existing non-MCP tool execution remains unchanged.
