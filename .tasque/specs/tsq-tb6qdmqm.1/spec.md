## Overview
Define the public sub-agent types, profile/config model, input modes, and prompt policy.

Primary files:
- `crates/roci-core/src/agent/subagents/mod.rs`
- `crates/roci-core/src/agent/subagents/types.rs`
- `crates/roci-core/src/agent/subagents/prompt.rs`
- `crates/roci-core/src/agent/mod.rs`

## Interfaces
- `SubagentId`
- `SubagentKind`
- `SubagentProfile`
- `SubagentProfileRef`
- `SubagentSupervisorConfig`
- `ModelCandidate`
- `ToolPolicy`
- `SubagentSpec`
- `SubagentInput`
- `SnapshotMode`
- `SubagentContext`
- `SubagentOverrides`
- `SubagentEvent`
- `SubagentStatus`
- `SubagentRunResult` / summary types
- `SubagentPromptPolicy`

## Constraints / Non-goals
- Keep types core-owned and CLI-agnostic.
- Do not implement runtime spawn logic here.
- Do not change `ask_user` request/response wire shapes in this task.
- Design for TOML-backed profile loading, but do not implement file loading here.

## Acceptance Criteria
- Public types compile and are re-exported from `roci_core::agent`.
- Types support named profiles and ordered model fallback candidates.
- Types support prompt-only, snapshot-only, and prompt+snapshot child inputs.
- Built-in profile set is `developer`, `planner`, and `explorer`.
- Types support profile versioning (`version = 1`) and single-parent inheritance semantics.
- Supervisor config can express bounded concurrency and related v1 guardrails.
- Prompt policy supports built-in defaults plus later profile/user overrides.

## Test Plan
- Unit tests for prompt builder and basic serde/clone/debug behavior where relevant.
