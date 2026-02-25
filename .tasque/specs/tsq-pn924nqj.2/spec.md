# tsq-pn924nqj.2: Tool execution update streaming end-to-end

## Problem
ToolExecutionUpdate exists in event/types surface, but updates are not emitted from loop execution path.

## Required behavior
- Runner must use execute_ext with update callback plumbing.
- Incremental updates must flow to event sink in-order between tool start and completion.
- Cancellation and error paths must terminate cleanly while preserving deterministic final events.
- Tools without execute_ext override must continue to work through the basic execute path.

## Test Plan
1. Stub tool emitting periodic updates; assert update stream payload and ordering.
2. Cancellation test while tool is emitting updates.
3. Regression tests for standard tool execution semantics and final result parity.
