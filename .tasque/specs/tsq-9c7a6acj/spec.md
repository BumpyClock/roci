# Resource loader + context files parity

## Goal
Achieve pi-mono parity (non-TUI) for resource discovery:
- Context files (AGENTS.md / CLAUDE.md)
- System prompt (SYSTEM.md)
- Append system prompt (APPEND_SYSTEM.md)
- Prompt templates (prompts/*.md + explicit paths)

## Defaults / Paths
- Global dir: ~/.roci/agent
- Project dir: .roci
- Prompts live under <dir>/prompts (non-recursive)
- Settings file: <dir>/settings.json (global + project, deep-merged)

## Out of scope
- Extensions/skills/themes loading (separate epics)
- TUI integration

## Deliverables
- ResourceLoader API + DefaultResourceLoader
- Prompt template expansion with args
- CLI integration and docs
