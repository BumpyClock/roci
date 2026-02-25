# SoC migration hardening: docs, tests, upgrade checklist

## Scope
- Docs for new crate boundaries + final import paths.
- Test matrix covering core + CLI + tools.
- Final checklist verifying strict SoC goals.

## Acceptance criteria
1) Docs updated (architecture + read_when links) to reflect final state.
2) Workspace tests pass (`cargo test -p roci`, `cargo test -p roci-cli`, `cargo test -p roci-tools`).
3) Checklist confirms: no clap in core, no CLI text in core errors, no stdout/exit in core, binary name `roci-agent`, tools import path `roci_tools::builtin`.
