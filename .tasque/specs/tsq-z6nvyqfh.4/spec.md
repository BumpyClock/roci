# Auto-compaction cancel semantics

## Scope
- Treat `SessionBeforeCompactOutcome::Cancel` in auto-compaction as run failure
- Emit explicit lifecycle failure message identifying hook cancellation
- Manual `compact()` cancellation returns explicit error (not silent no-op)

## Acceptance
- auto-compaction cancel fails run deterministically
- manual compact cancel returns deterministic error
- regression tests cover both paths
