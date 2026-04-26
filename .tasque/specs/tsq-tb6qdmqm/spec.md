## Overview
Implement sub-agent support in `roci-core` as a supervisor layer on top of existing `AgentRuntime`.

Reference design:
- `.ai_agents/sub_agents_plan.md`

## Scope
- named built-in + TOML-backed sub-agent profiles
- ordered model fallback candidates across providers
- prompt / snapshot / prompt+snapshot child input modes
- read-only context propagation
- background child spawn/lifecycle APIs
- parent-facing child event stream with origin metadata
- parent-mediated `ask_user` reuse through shared `UserInputCoordinator`
- parallel orchestration helpers, watch APIs, and guardrails
- tests, example harness, docs

## Non-goals
- no child-to-child peer bus
- no persistent message store / DB
- no TUI/session tree work
- no rewrite of `ask_user` tool contract
- no public out-of-process launcher API in v1

## Acceptance Criteria
- Parent harness can spawn one or many children without blocking.
- Child behavior can be driven by named profiles resolved from built-ins and TOML config.
- Profile model selection supports ordered provider/model fallback candidates.
- Child spawn supports prompt-only, snapshot-only, and prompt+snapshot modes.
- Default helper path is `PromptWithSnapshot + SummaryOnly`.
- Forwarded child events include `subagent_id` and lifecycle metadata.
- Child `ask_user` reaches the parent host and resumes correctly on response.
- Parent harness can `watch_snapshot`, `wait`, `wait_any`, `wait_all`, and abort active children.
- Tests and docs cover the shipped v1 contract and deferred peer-bus seam.
