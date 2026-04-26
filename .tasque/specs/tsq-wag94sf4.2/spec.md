## Context
Source note: /Users/adityasharma/Projects/roci/.ai_agents/codex_agent_loop.md

## Findings
- Codex loop relies on a first-class planning tool.
- roci built-in tool surface currently lacks update_plan parity.

## Scope guardrail
- Do not add update_plan just for parity.
- Add planning support only if there is a clear runtime/event/state model that fits roci.

## Acceptance
- Decide whether planning belongs in roci-tools builtin set or another layer.
- Define storage/event model for plans if proceeding.
- Add tests/docs for invocation and persistence semantics if implemented.
