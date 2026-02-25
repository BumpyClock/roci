# Refactor core auth into pure service APIs (no terminal/exit)

## Scope
- Remove terminal I/O + `process::exit` from auth flows in core.
- Provide a service API that returns typed steps/states.

## Implementation notes
- Create an `AuthService` (or similar) in core:
  - `start_login(provider)`
  - `poll_login(provider, session)`
  - `exchange_code(...)`
  - `status()` / `logout()`
- Return typed results (e.g., `AuthOutcome`, `AuthNextStep`).
- CLI handles prompt loops, printing, and exit codes.

## Acceptance criteria
1) Core auth APIs return typed results/errors only.
2) No stdout/stderr/exit in core auth modules.
3) Auth providers retain functionality.
4) Auth tests updated to service-level API.
