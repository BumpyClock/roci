# Steering + Follow-up Regression Tests

## Goal
Cover interruptions and follow-up continuation behaviors as dedicated regressions.

## Scope
- Steering during parallel tool batch.
- Steering during sequential tool execution.
- Follow-up restart after natural completion and multi-round follow-ups.

## Acceptance Criteria
- Remaining tools are skipped correctly when steering arrives.
- Follow-up rounds continue/terminate exactly as specified.
