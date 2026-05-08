## Overview
Lock the multi-server MCP identity and namespacing contract before implementation expands scope.

## Scope
- Confirm canonical tool identity, server identity, and aggregated routing shapes.
- Define how resources carry server provenance.
- Define the long-term exposed naming contract for multi-server MCP tools.

## Constraints / Non-goals
- Do not route by parsing exposed tool names.
- Do not use display labels as stable server identity.
- Do not make suffixing the default collision behavior.
- Do not change native Roci tool names unless a host explicitly opts into native namespacing.

## Interfaces (CLI/API)
```rust
pub enum McpToolCollisionPolicy {
    DenyOnCollision,
    SuffixOnCollision { hash_len: usize },
}

pub struct McpToolIdentity {
    pub server_id: String,
    pub tool_name: String,
}

pub struct ExposedMcpToolName {
    pub exposed_name: String,
    pub identity: McpToolIdentity,
}
```

## Decision
- Canonical exposed MCP tool name is `mcp__<server_id>__<tool_name>`.
- This is the intended long-term contract, not a transitional compatibility shim.
- Native Roci tools keep their existing plain names unless a host explicitly namespaces them.
- Runtime routing never reparses the exposed tool name; tool calls carry structured identity with raw `{ server_id, tool_name }`.
- Resource routing uses structured identity (`server_id`, `uri`) instead of encoded names.
- Callers should consume metadata structs rather than reparsing exposed names.
- Collision policy is explicit:
  - default `DenyOnCollision`, which fails fast when two raw identities normalize to the same exposed name
  - optional `SuffixOnCollision { hash_len: 12 }`, which appends a deterministic hash suffix for host/app compatibility

## Identity serialization details
`server_id` is a stable host-supplied identity, not a display label. It must be non-empty, unique per aggregate, stable across reconnect/session restart, and compatible with model-facing tool-name constraints. `server_label` is display-only.

V1 exposed-name serialization is exactly `mcp__{server_id}__{tool_name}`. Routing stores raw `{ server_id, tool_name }`; no runtime path reparses this string.

Collision detection compares final exposed names after serialization. `DenyOnCollision` returns a configuration error before exposing tools. `SuffixOnCollision { hash_len }` appends `__h{hash}`, where `hash` is lower-hex SHA-256 of `server_id + "\0" + tool_name`, truncated to `hash_len`.

## Data model / schema changes
- Add explicit stable server identity fields wherever aggregate tools, resources, instructions, auth, and transport state cross boundaries.
- Preserve raw `McpToolIdentity` beside model-visible exposed names.
- Store collision policy as aggregate configuration, defaulting to `DenyOnCollision`.
- Resource descriptors carry `{ server_id, uri }` provenance without encoding the server id into the URI.

## Acceptance criteria
1. Server identity fields are consistent across transports, auth, tools, instructions, and resources.
2. Namespacing decisions are documented clearly enough that follow-on tasks do not need to revisit them.
3. The task records why `mcp__<server_id>__<tool_name>` is preferred over alternatives such as `<server_id>__<tool_name>`, `server::tool`, or synthetic opaque ids.
4. `tsq-p4cpczyg.2` and `.3` can implement against this contract without additional design work.
5. Tests or design fixtures cover native/MCP name separation and both collision policy variants.

## Validation
- Design-only task: verify downstream task specs reference the same naming and identity model.

## Test plan
- Fixture tests for `mcp__<server_id>__<tool_name>` serialization.
- Collision tests for `DenyOnCollision`.
- Collision tests for `SuffixOnCollision { hash_len: 12 }` using lower-hex SHA-256 suffixing.
- Native/MCP separation tests proving native tool names remain plain.
- Resource provenance tests proving structured `{ server_id, uri }` routing.
