## Overview
Audit roci's current agent runtime against the desired sub-agent/parallel-sub-agent design, update `.ai_agents/sub_agents_plan.md`, and produce an implementation-ready backlog.

Primary references:
- `.ai_agents/sub_agents_plan.md`
- `docs/architecture/ask-user-v1-peer-bus-seam.md`
- `docs/ARCHITECTURE.md`

## Findings To Capture
- Reuse existing blocking `ask_user` path instead of rewriting it.
- Build v1 sub-agents as a supervisor layer on top of `AgentRuntime`.
- Defer generic envelope/store/router until a second real inter-agent message type exists.

## Deliverables
- Updated design doc in `.ai_agents/sub_agents_plan.md`.
- One implementation epic with dependency-ordered child tasks.
- Specs attached to the epic and each coding task.
- Duplicate/overlapping planning tasks resolved if their scope is subsumed here.

## Acceptance Criteria
- Design doc reflects the chosen v1 architecture.
- Implementation backlog is ready for coding agents without needing this chat.
- Dependencies make parallel execution explicit.
- No blocking architecture questions remain for v1.
