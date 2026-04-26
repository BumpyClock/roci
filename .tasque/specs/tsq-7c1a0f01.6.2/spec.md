## Overview
Define a consistent policy and implementation strategy for `result_large_err` warnings.

## Constraints / Non-goals
- Do not introduce inconsistent error handling patterns across crates.
- Avoid needless churn in stable APIs unless policy requires it.

## Interfaces (CLI/API)
- Target warning: `clippy::result_large_err`
- Candidate approaches: boxing error type at boundaries or scoped allow with documented policy.

## Data model / schema changes
No data/schema changes.

## Acceptance criteria
- Policy decision is documented and applied consistently to affected functions.
- Remaining occurrences are intentional and justified.

## Test plan
- Re-run clippy in affected crates/examples.
- Run tests that cover error paths where signatures changed.
