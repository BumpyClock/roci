## Overview
Add parallel orchestration helpers, watch/wait APIs, and supervisor guardrails for many active children.

Primary files:
- `crates/roci-core/src/agent/subagents/supervisor.rs`
- `crates/roci-core/src/agent/subagents/types.rs`

## Interfaces
- `wait_any()`
- `wait_all()`
- `list_active()`
- `watch_snapshot()` support on handles
- max-concurrency guardrails
- `max_active_children`
- supervisor-level shutdown / abort-all semantics if needed

## Constraints / Non-goals
- Keep APIs async-friendly and harness-first.
- Guardrails should be deterministic and core-owned.
- Do not add persistence in this task.

## Acceptance Criteria
- Parent can orchestrate multiple children concurrently through core APIs.
- Supervisor enforces active-child invariants and cleans up terminal children once.
- Abort/shutdown semantics are well-defined.
- Watch and terminal wait APIs are both available and useful for orchestration.

## Test Plan
- Runtime tests for concurrent child execution and `wait_any`/`wait_all` behavior.
