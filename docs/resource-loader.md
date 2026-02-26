# Resource Loader

Roci's resource loader composes three core concerns:

- settings (`~/.roci/agent/settings.json`, `.roci/settings.json`)
- context files (`AGENTS.md` / `CLAUDE.md`)
- prompt templates (`prompts/*.md`)

Project settings override global settings via deep merge.

## Context discovery

- Global root: `~/.roci/agent`
- Project roots: walk from filesystem root to current working directory
- Per-directory precedence: `AGENTS.md` first, fallback to `CLAUDE.md`
- Ordering: global first, then ancestors root -> cwd
- Paths are deduplicated

## System prompt files

- Base system prompt: `.roci/SYSTEM.md`, fallback `~/.roci/agent/SYSTEM.md`
- Appended prompt: `.roci/APPEND_SYSTEM.md`, fallback `~/.roci/agent/APPEND_SYSTEM.md`

CLI prompt assembly order:
1. `--system` if provided, otherwise discovered `SYSTEM.md`
2. discovered `APPEND_SYSTEM.md`
3. a single `Project Context` section containing discovered context files

## Prompt templates

Templates are loaded non-recursively from:

- `~/.roci/agent/prompts/*.md`
- `.roci/prompts/*.md`
- explicit configured prompt paths (file or directory)

Project templates override global templates with the same command name.

Slash expansion (`/<name>`) supports:

- `$1`, `$2`, ...
- `$@`
- `$ARGUMENTS`
- `${@:N}`
- `${@:N:L}`

Inputs that do not match a known template are passed through unchanged.

## Diagnostics

Resource loading returns diagnostics for unreadable files and collisions.
`roci-agent` surfaces these diagnostics to stderr.
