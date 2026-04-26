# Hook payload: cancellation token

## Scope
- Add cancel signal/token to `SessionBeforeCompactPayload`
- Thread signal from manual `compact()` and auto-compaction paths
- Ensure signal cancellation can terminate long-running hook work

## Acceptance
- payload exposes cancellation signal
- signal is wired in both execution paths
- test proves signal triggers cancellation behavior