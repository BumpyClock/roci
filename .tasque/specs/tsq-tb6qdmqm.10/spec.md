## Overview
Implement child input modes and read-only context snapshot propagation.

Primary files:
- `crates/roci-core/src/agent/subagents/context.rs`
- `crates/roci-core/src/agent/subagents/supervisor.rs`
- `crates/roci-core/src/agent/subagents/prompt.rs`

## Interfaces
- `SubagentInput::{Prompt, Snapshot, PromptWithSnapshot}`
- `SnapshotMode::{SummaryOnly, SelectedMessages, FullReadonlySnapshot}`
- `SubagentContext`
- parent helpers/builders for common spawn modes

## Constraints / Non-goals
- Context propagation is read-only.
- Do not introduce shared mutable parent/child message state.
- Snapshot-only mode should be explicit, not the default helper path.

## Acceptance Criteria
- Parent can spawn child with prompt-only, snapshot-only, or prompt+snapshot inputs.
- Snapshot propagation is structured and bounded.
- Prompt policy composes correctly with chosen input mode.
- Default helpers prefer prompt+snapshot.
- Default helper path is `PromptWithSnapshot + SummaryOnly`.
- `SelectedMessages` selection is explicit, not heuristic.
- `FullReadonlySnapshot` excludes transient runtime internals and mutable live state.

## Test Plan
- Unit/integration tests for all input modes and snapshot materialization rules.
