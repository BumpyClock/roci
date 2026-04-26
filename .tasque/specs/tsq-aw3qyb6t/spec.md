# pi-agent Core Parity Follow-ups (Non-TUI)

## Context
Recent roci alignment implemented async steering/follow-up hooks, queue drain modes, retry-delay caps, transport plumbing into ProviderRequest, and continue-without-input behavior.

## Remaining Objectives
1. Remove routing ambiguity in model selector codex heuristics.
2. Add extensible message conversion path equivalent to pi-agent convertToLlm flexibility.
3. Add runtime mutability APIs for long-lived agent orchestration.
4. Add fine-grained queue management APIs.
5. Make transport preference behaviorally effective (not just plumbed).

## Non-goals
- Any TUI/UI parity work.
- Broad provider rewrites beyond transport contract and selected implementation.

## Acceptance Gate
- New/updated tests pass for selector, runtime, runner, and affected providers.
- API/docs updated for any new public runtime hooks.
- No regressions in existing agent_loop and runtime test suites.
