# Event Ordering + Lifecycle Tests

## Goal
Lock event sequence invariants for run and turn lifecycle.

## Scope
- Assert `AgentStart -> TurnStart -> ... -> TurnEnd -> AgentEnd` ordering.
- Cover success, tool-turn, and error terminal paths.

## Acceptance Criteria
- Event sequence assertions deterministic and non-flaky.
- Regressions in ordering fail with clear diagnostics.
