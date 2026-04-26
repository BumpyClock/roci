# Dynamic API Key Resolution

## Goal
Support per-request async API key retrieval to allow token rotation and tenant-specific credentials.

## Scope
- Add `get_api_key` async callback hook to runtime/provider request path.
- Resolve key at request time, not agent construction time.
- Preserve existing static key path as fallback.
- Add tests for success/failure/empty-key behavior.

## Files
- `src/agent/agent.rs`
- `src/provider/mod.rs` or provider request builder location
- related provider implementations that consume request auth config

## Acceptance Criteria
- Callback invoked once per provider request.
- Callback error maps to actionable `RociError`.
- Static config still works unchanged when callback not set.
