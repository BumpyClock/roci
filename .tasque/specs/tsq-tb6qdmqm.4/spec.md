## Overview
Implement `SubagentSupervisor` and `SubagentHandle` spawn lifecycle on top of `AgentRuntime`.

Primary files:
- `crates/roci-core/src/agent/subagents/supervisor.rs`
- `crates/roci-core/src/agent/subagents/handle.rs`
- `crates/roci-core/src/agent/subagents/launcher.rs` (or equivalent internal module)
- `crates/roci-core/src/agent/subagents/mod.rs`
- `crates/roci-core/src/agent/mod.rs`

## Interfaces
- `SubagentSupervisor::new(...)`
- `SubagentSupervisor::spawn(...) -> SubagentHandle`
- `SubagentHandle::{id,label,watch_snapshot,abort,wait}`
- in-memory active child registry
- internal child launcher/factory seam used by the supervisor

## Constraints / Non-goals
- Build on `AgentRuntime`; do not create a second agent loop.
- Spawn must return immediately while child runs in a background task.
- Keep the launcher seam internal; do not expose unnecessary abstraction in the public v1 API.
- Rich event forwarding lands in `.5`; this task focuses on lifecycle and launch.

## Acceptance Criteria
- Parent can spawn a child and continue work.
- Parent can watch, wait for, or abort a child by handle.
- Active child registry is kept consistent across completion/failure/abort.
- The actual selected model candidate can be surfaced on launch/state.

## Test Plan
- Runtime tests for spawn/watch/wait/abort lifecycle.
