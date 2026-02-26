# ADR: CLI / core separation of concerns

## Goal
Capture the final SoC decision: `roci` core is UI-agnostic; `roci-cli` owns CLI UX; `roci-tools` owns built-in coding tools.

## Scope
- Create `docs/architecture/cli-soc.md` with:
  - Context/problem statement
  - Decision summary (from `tsq-jwv1ysxb.1`)
  - Consequences (binary rename, tool path, error mapping)
  - Ownership boundaries per crate
- Add `read_when` entry in `docs/learned/LEARNINGS.md` pointing to the ADR.

## Acceptance criteria
1) ADR file exists at `docs/architecture/cli-soc.md`.
2) `docs/learned/LEARNINGS.md` updated with a `read_when` link.
3) ADR content reflects decisions in `tsq-jwv1ysxb.1`.
