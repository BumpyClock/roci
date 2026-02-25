# SoC decisions + ADR for CLI/core boundary migration

## Decisions (approved)
1) **Breaking change policy**: remove `roci::cli` + `cli` feature immediately.
2) **Workspace layout**: root crate `roci` + `crates/roci-cli` member.
3) **CLI crate name**: `roci-cli`.
4) **Binary name**: `roci-agent`.
5) **Builtin tools extraction**: do now; crate name `roci-tools`.
6) **Auth API shape**: low-level primitives + `AuthService` facade.
7) **Error taxonomy**: typed missing-credential/config variants; CLI maps help text.
8) **Docs**: add architecture doc + read_when entry.

## Output
- ADR-style note recording the above decisions + rationale.
- Update downstream task specs to align with these decisions.

## Acceptance criteria
- ADR committed and linked from docs index.
- All downstream tasks updated with decision references.
