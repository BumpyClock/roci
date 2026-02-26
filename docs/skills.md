# Skills

Roci can discover skill definitions from the filesystem and inject a skills block into the system prompt.

## Locations and precedence

Skills are discovered from ordered roots and optional explicit paths. Precedence is:

1. Explicit paths (`--skill-path`)
2. Project roots (cwd only): `.roci/skills`, then `.agents/skills`
3. Global roots: `~/.roci/skills`, then `~/.agents/skills`

When names collide, the first skill found wins and later collisions are reported as diagnostics.

## Discovery rules

- Only `SKILL.md` files are loaded.
- Hidden files/dirs and `node_modules` are skipped.
- Ignore rules from `.gitignore`, `.ignore`, and `.fdignore` are respected.
- Symlinked directories are followed.

## Skill file format

`SKILL.md` must contain YAML frontmatter:

```yaml
---
name: my-skill
description: Short description of when to use the skill
disable-model-invocation: false
---
```

Validation rules:

- `name` is optional; defaults to the parent directory name
- `name` must be lowercase `a-z0-9-`, <= 64 chars, no leading/trailing `-`, no `--`
- `description` is required and <= 1024 chars

## CLI flags

- `--skill-path <PATH>`: explicit skill file or directory (repeatable)
- `--skill-root <PATH>`: additional root directory (repeatable)
- `--no-skills`: disable skill loading

## Library API

The core API accepts ordered roots and explicit paths via `roci_core::skills::LoadSkillsOptions` so embedding applications can define their own precedence rules.
