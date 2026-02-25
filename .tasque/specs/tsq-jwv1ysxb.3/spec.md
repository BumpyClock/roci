# Migrate CLI entrypoints/commands into roci-cli crate

## Scope
- Move `src/main.rs` + `src/cli/*` to `crates/roci-cli`.
- Remove `pub mod cli` + clap dependency from core.
- Rename CLI binary to `roci-agent`.

## Implementation notes
- Update paths/namespaces for CLI modules.
- Move CLI tests (arg parsing) to `roci-cli`.
- Update usage/help text to reference `roci-agent`.

## Acceptance criteria
1) CLI commands compile/run from `roci-cli`.
2) `roci-agent` binary behavior matches current CLI flows.
3) No clap usage remains in core crate.
4) Core library public API has no CLI module.
