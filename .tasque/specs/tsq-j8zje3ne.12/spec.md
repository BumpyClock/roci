# Session ID Propagation for Prompt Caching

## Goal
Thread `session_id` through agent/runtime/provider request path to enable provider-side prompt caching and session affinity.

## Scope
- Add session_id on run request metadata or typed field.
- Ensure provider request includes session id where supported.
- Keep no-op behavior for providers that do not support session cache/session ids.
- Add tests asserting propagation in provider request fixtures/mocks.

## Files
- `src/agent/agent.rs`
- `src/agent_loop/runner.rs`
- provider request conversion files

## Acceptance Criteria
- Session ID set on agent appears in outgoing provider request payload/options.
- No provider regressions when session ID absent.
