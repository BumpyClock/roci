# Skill discovery roots + diagnostics

## Scope
- Discover SKILL.md using configured roots; support:
  - global: `~/.roci/skills` and `~/.agents/skills`
  - project-local: `.roci/skills` and `.agents/skills` (cwd only)
- Load both global + local. On name collision, local wins.
- Explicit paths override local on name collision.
- Respect global config store order (customized via roci-core APIs). Ask user to confirm order when coding.
- Allow explicit skill paths (file/dir) in config/CLI.
- Scan with ignore rules (`.gitignore`, `.ignore`, `.fdignore`) like pi-mono.
- Deduplicate by file path; handle collisions with diagnostics.
- Follow symlinks.

## Acceptance
- Deterministic ordering with precedence: explicit > project (.roci then .agents) > global (~/.roci then ~/.agents), subject to config store override.
- Diagnostics include collisions and invalid/missing frontmatter.
- Tests for ignore behavior + symlink traversal.

## Notes
- No dotfiles repo default path.
