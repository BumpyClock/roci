## Context
Source note: /Users/adityasharma/Projects/roci/.ai_agents/codex_agent_loop.md

## Findings
- Provider-agnostic compaction is acceptable for roci; do not require /responses/compact parity.
- Current compaction rewrites history into a visible summary message.
- reasoning/thinking is not preserved as a durable transcript item in the inspected path.
- Exact prompt-prefix stability is not guaranteed after compaction and can also be affected by context hooks/sanitization.

## Scope guardrail
- Do not replace provider-agnostic compaction just to match Codex.
- Focus on documenting the tradeoff and deciding whether reasoning/prefix preservation matters enough to improve.

## Acceptance
- Document why provider-agnostic compaction is the default design.
- Decide whether to preserve more reasoning metadata or prefix stability where practical.
- Add follow-up implementation tasks only if the benefits justify complexity.
