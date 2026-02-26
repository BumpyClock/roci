# Align skill root precedence with config store

## Scope
- Change `default_skill_roots` to derive from `ResourceDirectories` (global + project) instead of hard-coded cwd/home.
- Use ResourceDirectories resolution to respect config store overrides.
- Precedence order stays explicit > project > global.
- Update CLI to use new `default_skill_roots` signature.
- Update tests + docs (`docs/skills.md`) to reflect new defaults.

## Open
- How to derive `.agents/skills` from `agent_dir` override (see question in chat).
