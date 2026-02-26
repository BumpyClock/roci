# SoC decisions + downstream task alignment

## Decisions (approved)
1) **Breaking change policy**: remove `roci::cli` + `cli` feature immediately.
2) **Workspace layout**: root crate `roci` + `crates/roci-cli` member.
3) **CLI crate name**: `roci-cli`.
4) **Binary name**: `roci-agent` only (no `roci` alias).
5) **Builtin tools extraction**: do now; crate name `roci-tools`.
6) **Builtin tools import path**: `roci_tools::builtin` (no compatibility shim).
7) **Auth API shape**: low-level primitives + `AuthService` facade.
8) **Error taxonomy**: typed missing-credential/config variants; CLI maps help text.
9) **Docs**: final-state docs, no upgrade notes.

## Scope
- Ensure all downstream task specs reflect the above decisions.
- Document any decision clarifications needed for implementation (but **ADR is owned by `tsq-jwv1ysxb.8`**).

## Acceptance criteria
1) Downstream task specs updated to match the decisions above.
2) No ADR creation steps remain in this task (handled by `tsq-jwv1ysxb.8`).
