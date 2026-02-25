# Strict SoC migration: split roci core from CLI surfaces

## Goal
Enforce a hard boundary where `roci` is a pure SDK and `roci-cli` is a consumer-only UX layer. Core must be UI-agnostic (no clap, no stdout/stderr, no process::exit, no CLI-specific error strings).

## Decisions (locked)
- Breaking change allowed: remove `roci::cli` module + `cli` feature immediately.
- Workspace layout: root crate `roci` + member `crates/roci-cli`.
- CLI crate name: `roci-cli`.
- Binary name: `roci-agent` (replace `roci`).
- Built-in tools extraction: **do now**, new crate `roci-tools`.
- Auth API shape: low-level primitives + `AuthService` facade returning typed states.
- Error taxonomy: typed missing-credential/config variants; CLI maps to help text.
- Docs: add architecture doc + read_when entry.

## Scope
- Extract CLI into `crates/roci-cli`.
- Move CLI orchestration/auth/arg parsing out of core.
- Convert auth to pure service APIs.
- Make core errors CLI-agnostic with typed metadata.
- Extract builtin tools into `roci-tools`.
- Docs + test matrix updates.

## Non-goals
- Provider feature expansion.
- TUI or other UI parity changes.
- Breaking changes beyond the CLI boundary cleanup.

## Definition of Done
- Core crate exposes only SDK modules (no CLI module, no clap).
- CLI builds as separate crate and provides `roci-agent` binary.
- Auth + error flow are UI-agnostic in core.
- Built-in tools live in `roci-tools`.
- Documentation and upgrade notes published.
- Workspace tests pass for core + CLI + tools.
