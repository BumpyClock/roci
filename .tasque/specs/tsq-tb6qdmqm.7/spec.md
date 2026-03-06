## Overview
Add end-to-end tests and an example harness for the sub-agent supervisor API.

Primary files:
- `crates/roci-core/src/agent/runtime_tests/*`
- `examples/*` (new sub-agent example if appropriate)
- `docs/testing.md` (if test invocation guidance changes)

## Coverage
- background spawn
- concurrent children
- event forwarding with `subagent_id`
- child `ask_user` response flow
- watch/wait semantics
- guardrail behavior
- prompt-only / snapshot-only / prompt+snapshot modes
- profile resolution / selected model candidate behavior

## Constraints / Non-goals
- Prefer deterministic tests with fake providers/tools.
- Example should exercise core API, not CLI-only wiring.

## Acceptance Criteria
- The new harness API has focused runtime/integration coverage.
- There is at least one example or usage snippet showing parent spawn + wait/watch.
