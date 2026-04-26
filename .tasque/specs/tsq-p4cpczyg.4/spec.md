## Overview
Add explicit reconnect and backoff behavior for remote MCP client connections.

## Scope
- Define reconnect policy objects for remote transports.
- Handle session-expired / transport-closed / transient network failures.
- Invalidate cached session-derived views (tools/resources) when reconnecting.

## Non-goals
- No reconnect loops for stdio subprocesses in v1.
- No hidden infinite retry behavior.
- No auth token acquisition in this task (consume auth outcomes from task .5).

## API / contract
- Remote client lifecycle must expose distinct terminal outcomes: `recovered`, `needs_auth`, `failed`.
- Policy defaults:
  - immediate session refresh retry once
  - exponential backoff with jitter for transport failures
  - bounded max attempts
- Auth failures are terminal and surfaced to host instead of retried forever.

## Acceptance criteria
1. Remote clients recover from transient disconnects without process restart when policy allows.
2. Session-expired failures trigger a fast reinitialize path before normal backoff.
3. Exhausted retries and auth failures surface deterministic terminal states.
4. Reconnect tests prove cached tool/resource state is refreshed after reconnect.

## Validation
- Integration tests for recoverable disconnects, session-expired errors, exhausted retries, and auth-terminal failures.
