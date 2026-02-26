## Overview
Resolve or explicitly policy-handle design-level clippy warnings that require judgment and broader refactors.

## Constraints / Non-goals
- Avoid risky rewrites with unclear benefit.
- Any `#[allow(clippy::...)]` must be narrow and justified.

## Interfaces (CLI/API)
- Candidate warnings: `type_complexity`, `too_many_arguments`, `result_large_err`, `module_inception`, `mut_range_bound`, `needless_range_loop`, `incompatible_msrv`

## Data model / schema changes
None unless a warning fix explicitly requires API structure changes.

## Acceptance criteria
- For each design-level warning, choose one: fix, defer with rationale, or scoped allow with justification.
- Decisions are documented in task notes/spec updates.
- Remaining warnings after this task are intentional and explainable.

## Test plan
- Re-run full clippy and verify warning deltas.
- Run targeted tests for refactored modules.
