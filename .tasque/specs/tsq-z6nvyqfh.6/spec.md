# Hook tests: override/cancel/signal

## Scope
- Add tests for full override contract validation
- Add tests for auto-compaction cancel => run fail
- Add tests for manual compact cancel error
- Add tests for cancellation signal wiring

## Acceptance
- tests fail before implementation and pass after
- no regressions in existing runtime hook tests