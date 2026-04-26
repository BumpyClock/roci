## Overview
Add first-class MCP resource discovery and read APIs at the SDK layer, plus aggregated multi-server views.

## Scope
- Add low-level client methods for `resources/list` and `resources/read`.
- Add typed descriptor/content models with server provenance.
- Add aggregate helpers for multi-server resource listing and targeted reads.

## Non-goals
- No resource subscriptions in this task.
- No prompt/skill generation from resources in this task.
- Do not rewrite canonical upstream URIs.

## API / contract
- Single-server API:
  - `list_resources()` -> descriptors
  - `read_resource(uri)` -> content
- Aggregated API:
  - list returns sidecar server identity per resource
  - reads use structured routing (`server_id`, `uri`)
- Binary/text content handling must be explicit in the returned type.

## Acceptance criteria
1. SDK consumers can list and read resources from a single MCP server through typed APIs.
2. Multi-server aggregation returns deterministic ordering and preserves server provenance.
3. Resource read failures map cleanly without corrupting aggregator state.
4. Tests cover text and non-text resource payloads plus unknown-resource failures.

## Validation
- Integration tests with fixture servers exposing at least one text resource and one non-text/binary-like resource.
- Regression coverage for existing tool aggregation behavior.
