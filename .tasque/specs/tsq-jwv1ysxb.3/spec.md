# Migrate CLI entrypoints/commands into roci-cli crate

## Scope
- Move `src/main.rs` + `src/cli/*` to `crates/roci-cli`.
- Remove `pub mod cli` + clap dependency from core.
- Remove the `cli` feature from core `Cargo.toml`.
- Rename CLI binary to `roci-agent`.
- Consume AuthService API from core (implemented in .4).

## Dependencies
- Must run after `.2` (workspace scaffolding) and `.4` (auth refactor).

## Acceptance criteria
1) CLI commands compile/run from `roci-cli`.
2) `roci-agent` binary behavior matches current CLI flows.
3) No clap usage remains in core crate.
4) Core library public API has no CLI module.
5) `cargo test -p roci-cli` passes (existing parsing tests; no new integration test required).
