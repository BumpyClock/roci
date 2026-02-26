## Overview
Turn triage results into an ordered remediation plan with explicit execution phases and dependency rationale.

## Constraints / Non-goals
- No implementation changes in this task.
- Plan must keep independent workstreams parallelizable when safe.

## Interfaces (CLI/API)
- `tsq dep add <child> <blocker> --type blocks`
- `tsq dep add <later> <earlier> --type starts_after`
- `tsq ready --lane planning`

## Data model / schema changes
No code schema changes.

## Acceptance criteria
- Every warning category has a destination child task.
- Dependency graph is documented and consistent with intended order.
- Task specs/notes include clear done criteria for each child.

## Test plan
- Inspect tree with `tsq list --tree` and `tsq dep tree tsq-7c1a0f01 --direction down --depth 4`.
- Verify exactly one logical path to final verification task.
