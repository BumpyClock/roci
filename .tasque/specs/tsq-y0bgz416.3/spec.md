## Overview
Add multi-server MCP aggregation as a library primitive.

## Constraints / Non-goals
- Non-goal: CLI-specific fan-out orchestration.
- Must provide deterministic routing and conflict handling.

## Interfaces (CLI/API)
- Aggregator API that accepts multiple MCP server configs/clients and exposes a single dynamic tool provider surface.
- Deterministic tool name collision policy (configured behavior).
- Default collision policy is auto-namespace: `<server_id>__<tool_name>`.
- Multi-server initialization uses fail-fast strict default.

## Data model / schema changes
- Aggregated tool metadata includes origin server id.
- Routing map from exposed tool key -> backing server/tool identity.

## Acceptance criteria
1. Aggregator lists tools across N servers.
2. Execution routes to correct server/tool deterministically.
3. Collision policy behavior is explicit and tested (including namespace format).
4. Aggregator failures isolate per-server errors without corrupting global state.

## Test plan
- Unit tests for merge/routing/collision behavior.
- Integration tests with two mock MCP servers.
