## Overview
Implement built-in + TOML-backed sub-agent profile loading and effective profile resolution.

Primary files:
- `crates/roci-core/src/agent/subagents/profiles.rs`
- `crates/roci-core/src/agent/subagents/config.rs`
- `crates/roci-core/src/agent/subagents/mod.rs`
- `crates/roci-core/src/resource/*` or other config-loading area only if needed

## Interfaces
- `SubagentProfileRegistry`
- built-in profile registration
- TOML profile loading from configured roots
- effective profile resolution with inheritance/override rules
- ordered `ModelCandidate` fallback resolution policy

## Constraints / Non-goals
- Keep the file/config format core-owned and reusable by future harnesses.
- Support user-defined profiles and built-ins from the start.
- Do not implement remote provider health checking beyond startup-time candidate fallback.

## Acceptance Criteria
- Profiles can be loaded from TOML and merged with built-ins.
- Discovery roots/precedence follow the same config-store model as other roci config files.
- Effective profile resolution supports user/developer system prompt overrides.
- Model candidates support ordered cross-provider fallback with reasoning effort per candidate.
- Fallback happens only at launch/provider-acquisition time, never mid-run.
- Tool inheritance/override policy is resolved deterministically.
- Inheritance is single-parent only.
- Built-in shipped profiles are `developer`, `planner`, and `explorer`.

## Test Plan
- Unit tests for profile parsing, merge/inheritance rules, and candidate fallback ordering.
