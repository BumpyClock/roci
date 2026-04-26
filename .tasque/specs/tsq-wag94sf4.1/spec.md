## Context
Source note: /Users/adityasharma/Projects/roci/.ai_agents/codex_agent_loop.md

## Findings
- Current assembly path builds one merged system prompt from SYSTEM.md, APPEND_SYSTEM.md, rendered AGENTS/CLAUDE context, skills, and MCP instructions.
- Codex article describes distinct developer and user items plus a separate environment message before the actual user prompt.
- Current context discovery checks AGENTS.md then CLAUDE.md; no AGENTS.override.md support in inspected path.

## Scope guardrail
- Do not chase 1:1 parity.
- Only split prompt items if it materially improves request clarity, provider adapters, testability, or prompt-debug tooling.

## Acceptance
- Decide target prompt assembly model for roci.
- Implement explicit itemization only if justified.
- Add tests for ordering/resource behavior.
- Evaluate AGENTS.override.md / fallback filename support on its own merits.
