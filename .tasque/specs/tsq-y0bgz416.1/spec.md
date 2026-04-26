## Overview
Lock the contract for MCP library behavior before implementation to avoid rework.

## Constraints / Non-goals
- Non-goal: implement transport/runtime code in this task.
- Must remain backward-compatible where feasible with current public symbols.

## Interfaces (CLI/API)
- Define/confirm:
  - MCP server identity model (id, label, endpoint kind).
  - Tool collision policy contract.
  - Instruction exposure + merge helper signatures.
  - Aggregated provider interface expectations.
- Lock decisions in contract:
  - Collision policy default: auto-namespace (`<server_id>__<tool_name>`).
  - Multi-server init default: fail-fast strict.
  - Instruction merge default: append MCP instructions after existing system prompt.
  - Instruction labels in merged prompt are model-visible and formatted as `[server:<id>]`.
  - Reconnect/protocol fallback: included in implementation scope.

## Data model / schema changes
- New/updated Rust structs/enums for server descriptor, collision policy, and instruction aggregate.

## Acceptance criteria
1. Spec/doc sections define all above interfaces with examples.
2. Open questions are either resolved or explicitly deferred with rationale.
3. `tsq` spec check passes for this task.

## Test plan
- N/A (design/spec task), but include test matrix placeholders for downstream tasks.
