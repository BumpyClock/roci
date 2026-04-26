## Overview
Forward child events to the parent and reuse the shared `UserInputCoordinator` for parent-mediated `ask_user`.

Primary files:
- `crates/roci-core/src/agent/subagents/supervisor.rs`
- `crates/roci-core/src/agent/subagents/events.rs`
- `crates/roci-core/src/agent/mod.rs`

## Interfaces
- parent-facing subscription/event channel for `SubagentEvent`
- `SubagentSupervisor::submit_user_input(...)`
- forwarded event payloads include `subagent_id`, optional label, and lifecycle state

## Constraints / Non-goals
- Reuse existing `ask_user` tool/runtime path; do not rewrite it onto a generic bus.
- Keep child identity out-of-band in supervisor events rather than mutating the `ask_user` tool schema.
- No child-to-child messaging.

## Acceptance Criteria
- Parent receives forwarded child `AgentEvent`s with child metadata.
- `UserInputRequested` from a child can be answered through the supervisor and resumes the correct child.
- Shared coordinator behavior remains deterministic for timeout/cancel/unknown-request paths.

## Test Plan
- Integration tests for child `ask_user` -> parent response -> child resume.
