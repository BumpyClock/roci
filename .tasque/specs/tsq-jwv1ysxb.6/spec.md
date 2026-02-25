# Extract built-in coding tools into roci-tools crate

## Decision
- In scope now; crate name `roci-tools`.

## Scope
- Move `src/tools/builtin.rs` into `crates/roci-tools`.
- Core keeps tool traits/validation; tools live in `roci-tools`.
- CLI imports built-in tools from `roci-tools`.

## API compatibility
- New import path is `roci_tools::builtin`.
- No compatibility shim in `roci` core.
- Docs should reflect the final import path.

## Acceptance criteria
1) Built-in tool implementations live in `roci-tools`.
2) CLI uses tools from `roci-tools`.
3) Tests migrated to `roci-tools`.
4) Docs updated to show the `roci_tools::builtin` import path.
