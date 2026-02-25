# tsq-pn924nqj: Remaining pi-agent core parity closure

read_when:
- You need to finish non-TUI parity with pi-agent event semantics.

## Scope
1. Agent-level message lifecycle streaming events (start/update/end).
2. Tool execution incremental update streaming via callback path.
3. End-to-end validation and documentation hardening.

## Out of Scope
- TUI parity.
- Provider feature expansion unrelated to event/update parity.

## Definition of Done
- All child tasks closed with passing targeted + regression suites.
- Event ordering and payload contracts documented.
- Parity checklist confirms no open gaps in these two domains.
