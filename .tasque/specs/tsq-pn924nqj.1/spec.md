# tsq-pn924nqj.1: Agent message lifecycle events end-to-end

## Problem
Runner does not consistently emit message lifecycle events with pi-agent-like fidelity.

## Required behavior
- Emit lifecycle events for assistant output:
  - message start when assistant output begins
  - update on deltas (text/reasoning)
  - message end on terminal completion/cancel/failure with clear semantics
- Preserve compatibility with existing event sinks and payload types.

## Test Plan
1. Unit tests for ordering and payload fields.
2. Runner tests for complete/cancel/fail/tool-turn scenarios.
3. Backward compatibility checks for existing event consumers.
