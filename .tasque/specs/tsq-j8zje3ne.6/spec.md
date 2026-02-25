# Follow-up Message Handling

## Goal
Validate outer-loop behavior: follow-up messages restart the inner run only after a natural assistant completion boundary.

## Scope
- Add runner tests that queue follow-up messages after a no-tool turn.
- Verify follow-up checks are not applied mid-turn.
- Verify repeated follow-up rounds terminate once callback returns empty.

## Files
- `src/agent_loop/runner.rs` (tests module)

## Acceptance Criteria
- Natural turn end + queued follow-ups triggers another inner-loop round.
- Follow-up messages appear in conversation before subsequent provider call.
- Multiple follow-up rounds preserve ordering and stop correctly.
