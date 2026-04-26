# Create workspace scaffolding for crates/roci-cli + roci-tools

## Scope
- Convert repo into a workspace with `roci` (core) + `crates/roci-cli` + `crates/roci-tools`.
- Keep existing CLI behavior intact aside from binary name change.

## Implementation notes
- Add `[workspace]` to root `Cargo.toml` and include `crates/roci-cli` and `crates/roci-tools`.
- Create `crates/roci-cli/Cargo.toml` with dependency on `roci`.
- Create `crates/roci-tools/Cargo.toml` (empty for now, to be filled in task .6).
- Binary name must be **`roci-agent`**.
- Ensure `roci-cli` depends on correct `roci` features (likely `agent` + provider defaults).

## Acceptance criteria
1) `cargo build -p roci` and `cargo build -p roci-cli` succeed.
2) `roci-agent` binary produced from CLI crate.
3) Core `roci` crate no longer owns CLI binary wiring.
4) Workspace includes `roci-tools` crate directory (even if empty for now).
5) CI/test commands updated to workspace-aware invocation.
