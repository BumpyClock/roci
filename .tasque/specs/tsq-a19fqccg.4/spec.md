# Branch summarization utilities

## Scope
- Collect entries between branches for summary
- Token-budgeted selection (newest-first)
- Summary generation using same structured format + file ops
- Cumulative file tracking across summaries
- Explicit only (triggered by tree navigation), not automatic
- Summary model selection: branch_summary.model if set, else run model

## Dependencies
- Requires session tree/navigation primitives (tsq-jhvzt78z)

## Acceptance
- Core utilities implemented + unit tests
- Integrates with session tree API once available
