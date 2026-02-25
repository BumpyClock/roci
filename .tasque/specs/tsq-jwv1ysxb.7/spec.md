# SoC migration hardening: docs, tests, upgrade checklist

## Scope
- Docs for new crate boundaries + upgrade notes.
- Test matrix covering core + CLI + tools.
- Final checklist verifying strict SoC goals.

## Acceptance criteria
1) Docs updated (architecture + read_when links).
2) Workspace tests pass (`cargo test -p roci`, `cargo test -p roci-cli`, `cargo test -p roci-tools`).
3) Upgrade/migration note for downstream users (binary rename `roci-agent`).
4) Checklist confirms: no clap in core, no CLI text in core errors, no stdout/exit in core.
