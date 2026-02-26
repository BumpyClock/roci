# Refactor core auth into pure service APIs (no terminal/exit)

## Scope
- Remove terminal I/O + `process::exit` from auth flows in core.
- Provide a service API that returns typed steps/states for both device-code and PKCE flows.

## Proposed types (guide, not strict)
- `AuthStart`:
  - `DeviceCode { verification_url, user_code, interval_secs, expires_at }`
  - `Pkce { authorize_url, state }`
- `AuthPoll`:
  - `Pending`
  - `SlowDown { interval_secs }`
  - `Authorized { token }`
  - `Denied | Expired`
- `AuthError`: typed failures (network, invalid_code, state_mismatch, refresh_failed, etc.)

## Responsibilities
- Core returns typed states and errors only.
- CLI owns prompting, printing, retry loops, and exit codes.
- Core may still persist tokens via TokenStore when authorization completes.

## Acceptance criteria
1) Core auth APIs return typed results/errors only (no stdout/stderr/exit).
2) AuthService supports: Copilot/Codex device-code + Claude PKCE.
3) Existing auth functionality preserved via CLI wrapper.
4) Tests updated to validate service-level behavior (no CLI output expectations).
