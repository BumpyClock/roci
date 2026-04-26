# Hooks: compaction/tree

## Decisions
- D1: `session_before_compact` supports full override object (not summary-only)
- D2: auto-compaction cancel fails the run with explicit error
- D3: compaction hook payload includes cancellation signal/token
- D4: tree hook instruction/label overrides deferred to session-tree epic

## Scope
- roci-core hook contract + execution semantics
- no extensions system required

## Out of scope
- extension loading/runtime
- tree instruction/label override controls

## Acceptance
- contract implemented + validated
- cancel semantics enforced
- tests/docs updated