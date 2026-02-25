# Create workspace scaffolding for crates/roci-cli

## Scope
- Convert repo into a workspace with `roci` (core) + `crates/roci-cli`.
- Keep existing CLI behavior intact aside from binary name change.

## Implementation notes
- Add `[workspace]` to root `Cargo.toml` and include `crates/roci-cli`.
- Create `crates/roci-cli/Cargo.toml` with dependency on `roci`.
- Binary name must be **`roci-agent`**.
- Ensure `roci-cli` depends on correct `roci` features (likely `agent` + provider defaults).

## Acceptance criteria
1) `cargo build -p roci` and `cargo build -p roci-cli` succeed.
2) `roci-agent` binary produced from CLI crate.
3) Core `roci` crate no longer owns CLI binary wiring.
4) CI/test commands updated to workspace-aware invocation.
