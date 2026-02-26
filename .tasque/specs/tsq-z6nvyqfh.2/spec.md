# Hook contract: full compaction override

## Scope
- Add result type: `summary`, `first_kept_entry_id`, `tokens_before`, optional `details`
- Accept override from `session_before_compact` for manual and auto paths
- Validate non-empty summary, valid kept index semantics

## Acceptance
- runtime accepts full override contract
- invalid overrides fail with descriptive `InvalidState` errors
- existing summary-only tests replaced/extended