# Extract built-in coding tools into roci-tools crate

## Decision
- In scope now; crate name `roci-tools`.

## Scope
- Move `src/tools/builtin.rs` into `crates/roci-tools`.
- Core keeps tool traits/validation; tools live in `roci-tools`.
- CLI imports built-in tools from `roci-tools`.

## API compatibility
- Prefer keeping an easy import path for SDK users.
- Decide whether to re-export `roci_tools::builtin` from `roci` (if feasible) or update docs/upgrade notes to new import path.

## Acceptance criteria
1) Built-in tool implementations live in `roci-tools`.
2) CLI uses tools from `roci-tools`.
3) Tests migrated to `roci-tools`.
4) Docs/upgrade notes clarify import path and compatibility story.
