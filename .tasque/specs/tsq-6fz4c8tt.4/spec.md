# CLI/config wiring for skill paths

## Scope
- Add CLI/config fields for skill roots and explicit skill paths.
- Defaults:
  - project-local: `.roci/skills` and `.agents/skills` (cwd only)
  - global: `~/.roci/skills` and `~/.agents/skills`
- Load both global + local. On name collision, local wins.
- Explicit paths override local on name collision.
- roci-core must accept injected roots/paths; no hard-coded paths in core.
- Include flags to disable skills loading.

## Acceptance
- CLI can list resolved skill roots (debug/log).
- roci-core API supports custom roots for embedding as library.

## Notes
- Precedence order subject to global config store override; confirm when coding.
