## Overview
Lock the multi-server MCP identity and namespacing contract before implementation expands scope.

## Scope
- Confirm canonical tool identity, server identity, and aggregated routing shapes.
- Define how resources carry server provenance.
- Define the long-term exposed naming contract for multi-server MCP tools.

## Decision
- Canonical exposed tool name remains `<server_id>__<tool_name>`.
- This is the intended long-term contract, not a transitional compatibility shim.
- Resource routing uses structured identity (`server_id`, `uri`) instead of encoded names.
- Callers should consume metadata structs rather than reparsing exposed names.

## Acceptance criteria
1. Server identity fields are consistent across transports, auth, tools, instructions, and resources.
2. Namespacing decisions are documented clearly enough that follow-on tasks do not need to revisit them.
3. The task records why `<server_id>__<tool_name>` is preferred over alternatives such as `server::tool` or synthetic opaque ids.
4. `tsq-p4cpczyg.2` and `.3` can implement against this contract without additional design work.

## Validation
- Design-only task: verify downstream task specs reference the same naming and identity model.
