## Goal
Plan and lock the next MCP SDK layer for Roci without changing implementation yet.

## Overview
This epic defines the MCP SDK layer for Roci: typed transports, auth/resource boundaries, reconnect semantics, aggregate identity, and server-mode scope. Child tasks implement these contracts incrementally without re-deciding naming, ownership, or protocol boundaries.

## Product stance
- Roci is in active development with no external SDK users yet.
- Breaking MCP API changes are acceptable when they improve the long-term SDK shape.
- Prefer clean public naming and module boundaries over compatibility shims.

## Constraints / Non-goals
- Do not preserve old MCP remote-transport names as long-term public API when the new name is clearer.
- Do not let hosts parse exposed tool names for routing; structured identity is canonical.
- Do not move app-owned UX such as browser launches, prompts, or account selection into core SDK primitives.
- Do not proxy upstream MCP servers through Roci server mode in v1 unless a child task explicitly adds that scope later.

## Current baseline
- Existing Roci support already covers feature-gated MCP client wiring for stdio + rmcp streamable HTTP/SSE, single-server tool calls, multi-server tool aggregation, and instruction merging.
- Missing or partial for this epic: first-class HTTP naming/model, WebSocket transport, client resource APIs, auth discovery + token lifecycle, explicit reconnect policy, and MCP server mode.
- Prior art:
  - Claude Code has the deepest local prior art: stdio/SSE/HTTP/WS client configs, resource fetch, OAuth discovery/token handling, and reconnect heuristics; its MCP server mode is stdio-only and does not re-expose downstream MCP tools.
  - Pi explicitly states “No MCP”.
  - The local Codex CLI checkout is only a package/native-binary launcher, so there is no inspectable MCP source in this repo snapshot.

## Target architecture
### Container boundaries
- `mcp::transport`
  - Owns connection config, transport-specific connect/send/close mechanics, and transport-level retry knobs.
  - Public configs should distinguish `StdioTransport`, `StreamableHttpTransport`, and `WebSocketTransport`.
  - Rename the current remote HTTP transport surface to `StreamableHttpTransport`; do not keep `SSETransport` as a public long-term shim unless implementation discovers an internal-only migration need.
- `mcp::client`
  - Owns protocol session lifecycle: initialize, tool list/call, resource list/read, capability inspection, and session reset.
  - Must not own browser UX or app-level reconnection loops.
- `mcp::auth`
  - Owns OAuth discovery, token refresh/exchange primitives, and persistence interfaces.
  - Reuse `TokenStore` through an MCP-scoped adapter (`provider = mcp`, profile keyed by server id).
- `mcp::aggregate`
  - Owns multi-server identity, namespacing, routing, aggregated instructions, and aggregated resource views.
- `mcp::server`
  - Owns exposing Roci `Tool` / `DynamicToolProvider` surfaces as an MCP server service.
  - V1 scope is server-core plus stdio, HTTP, and WebSocket hosting adapters.
- Host / app layer
  - Owns interactive UX (open browser, device-code prompt, approval flows, reconnect UI, config persistence beyond token store wiring).

## Transport abstraction model
- Add a public connection descriptor layer that separates server identity from transport details.
- Canonical remote transport names:
  - `StreamableHttpTransport` for MCP streamable HTTP endpoints (even when the server uses SSE for event delivery).
  - `WebSocketTransport` for JSON-RPC over MCP WebSocket endpoints.
- Preserve stdio as the simplest local transport.
- Server mode should mirror this shape with host adapters for stdio, HTTP, and WebSocket rather than inventing a separate serving vocabulary.
- Do not add another general “transport manager” service unless transport behavior genuinely differs; keep a small trait boundary and typed config structs.

## Auth boundary (SDK vs app)
- SDK owns:
  - RFC 9728 protected-resource discovery.
  - RFC 8414 authorization-server metadata lookup.
  - PKCE/device-code helper primitives.
  - token refresh + expiry checks.
  - token persistence via existing `TokenStore` abstraction.
- App owns:
  - browser launch / callback listener UX.
  - device-code display / approval UX.
  - user approval prompts and account selection.
- Required SDK contract: auth flows must work with callback traits so CLI/TUI/GUI hosts can plug in UX without forking protocol logic.

## Interfaces (CLI/API)
- `StdioTransport`, `StreamableHttpTransport`, and `WebSocketTransport` are the public transport vocabulary.
- `MCPClient` owns initialize, tools/list, tools/call, resource list/read, capability inspection, and session reset.
- `MCPResourceDescriptor` carries structured `{ server_id, uri }` provenance.
- Aggregated MCP tools expose `mcp__<server_id>__<tool_name>` while routing uses structured identity.
- MCP server mode exposes Roci tools through a transport-agnostic server core plus thin stdio/HTTP/WebSocket host adapters.

## Resource model
- Add typed client APIs for:
  - `list_resources()` -> `Vec<MCPResourceDescriptor>`
  - `read_resource(uri)` -> `MCPResourceContents`
- `MCPResourceDescriptor` should include at minimum: `server_id`, `server_label`, `uri`, `name`, `description`, `mime_type`, and optional annotations.
- Aggregation rule: preserve upstream `uri` unchanged and attach server identity as sidecar metadata. Do **not** rewrite URIs globally in v1.
- Optional helper methods may expose synthetic lookup keys for UI use, but canonical read calls must still accept `(server_id, uri)`.

## Data model / schema changes
- Add explicit MCP server identity fields: stable `server_id` and display-only `server_label`.
- Add aggregate tool metadata that preserves raw `{ server_id, tool_name }` beside exposed names.
- Add resource descriptors with sidecar server provenance instead of URI rewriting.
- Add typed transport configs and shared remote auth/header/timeout fields.
- Add server-core request/response mapping types reusable by stdio, HTTP, and WebSocket hosts.

## Reconnection lifecycle
- Scope reconnect logic to remote transports only (`streamable-http`, `ws`).
- State machine:
  - `Disconnected` -> `Connecting` -> `Initialized`
  - `Initialized` -> `Retrying` on transport/session loss
  - `Retrying` -> `Initialized` on success
  - `Retrying` -> `NeedsAuth` on terminal auth failure
  - `Retrying` -> `Failed` after policy exhaustion
- Policy:
  - one immediate fast retry for “session expired / session not found”
  - exponential backoff with jitter for network failures
  - no infinite hidden retries
  - auth failures must short-circuit into `NeedsAuth`, not churn retries
- Reconnect must invalidate cached tool/resource views tied to the dead session.

## Server mode scope
- V1 server mode will expose Roci static tools and dynamic tool providers as MCP tools through a transport-agnostic server core plus stdio, HTTP, and WebSocket hosts.
- V1 non-goals:
  - proxying upstream MCP servers through Roci
  - multiplexing resources/prompts from external MCP servers
  - advanced production concerns such as multitenancy, distributed session coordination, or ingress-specific auth frameworks
- HTTP/WS hosting should be designed as thin adapters over the same server core, not parallel implementations.

## Namespacing contract
- Canonical MCP aggregate tool exposure is `mcp__<server_id>__<tool_name>`.
- Native Roci tools stay plain unless a host explicitly namespaces them.
- This is a fresh long-term decision, not a compatibility concession: it keeps names model/provider-safe while remaining readable and deterministic.
- Introduce explicit metadata structs so callers do not need to parse tool names for routing.
- Resource routing should use structured identity (`server_id`, `uri`) instead of encoded name strings.
- Collision policy defaults to `DenyOnCollision`, with optional deterministic suffixing for hosts that prefer graceful name disambiguation.

## Acceptance criteria
1. Epic child specs define contracts for transports, auth, resources, reconnect, server mode, and namespacing with no major ambiguity left for implementers.
2. Child tasks have explicit dependency edges and planning state `planned`.
3. Rollout order is clear enough for parallel implementation without re-deciding core contracts.
4. Open questions are isolated to true product/host decisions, not missing technical detail.

## Test plan
- Contract tests for exposed MCP naming, collision policies, and structured route identity.
- Transport tests for stdio, Streamable HTTP, and WebSocket initialize/list/call paths.
- Resource aggregation tests proving URI preservation plus server provenance.
- Server-core tests proving host adapters share listing, dispatch, and error mapping behavior.
- Regression tests proving stale remote transport names do not remain as long-term public API.

## Open questions
1. Should auth UX stay entirely host-provided, or does Roci need a default local callback helper for CLI apps?
2. For aggregated resources, do downstream consumers prefer explicit `(server_id, uri)` pairs or a synthetic display id in addition to canonical fields?
3. Does HTTP/WS server hosting need any v1 auth/allowlist boundary, or can it remain bind/listen only with host-supplied outer security?
