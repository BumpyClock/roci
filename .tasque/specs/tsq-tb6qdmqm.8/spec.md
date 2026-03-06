## Overview
Document the sub-agent supervisor API, named profile system, and deferred peer-bus seam.

Primary files:
- `.ai_agents/sub_agents_plan.md`
- `docs/ARCHITECTURE.md`
- `docs/learned/LEARNINGS.md` or a focused learned doc if durable pitfalls emerge

## Constraints / Non-goals
- Document final shipped behavior, not speculative v2 detail.
- Be explicit that peer bus is deferred by design, not forgotten.

## Acceptance Criteria
- Architecture docs explain supervisor-first design and `ask_user` reuse.
- Docs explain named profiles, model candidate fallback, and child input modes.
- Public docs tell future coding agents where sub-agent core APIs live.
- Any new testing/example entry points are documented.
