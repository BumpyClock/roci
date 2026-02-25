# SoC decisions + ADR for CLI/core boundary migration

## Decisions (approved)
1) **Breaking change policy**: remove `roci::cli` + `cli` feature immediately.
2) **Workspace layout**: root crate `roci` + `crates/roci-cli` member.
3) **CLI crate name**: `roci-cli`.
4) **Binary name**: `roci-agent` only (no `roci` alias).
5) **Builtin tools extraction**: do now; crate name `roci-tools`.
6) **Builtin tools import path**: `roci_tools::builtin` (no compatibility shim).
7) **Auth API shape**: low-level primitives + `AuthService` facade.
8) **Error taxonomy**: typed missing-credential/config variants; CLI maps help text.
9) **Docs**: add architecture doc + read_when entry; docs reflect final state (no upgrade notes).

## ADR requirement (new subtask)
- Create ADR at `docs/architecture/cli-soc.md` describing SoC boundary and crate responsibilities.
- Add `read_when` entry in `docs/learned/LEARNINGS.md`.

## Output
- ADR-style note recording the above decisions + rationale.
- Update downstream task specs to align with these decisions.

## Acceptance criteria
- ADR committed and linked from docs index.
- All downstream tasks updated with decision references.
