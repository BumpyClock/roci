## Goal
Align roci's OpenAI Responses/Codex session semantics closer to pi-mono.

## References
- Source note: /Users/adityasharma/Projects/roci/.ai_agents/codex_agent_loop.md
- pi-mono reference: /Users/adityasharma/Projects/references/pi-mono
- roci adapter: crates/roci-providers/src/provider/openai_responses.rs
- roci settings: crates/roci-core/src/types/generation.rs

## Current behavior in roci
- Supports explicit previous_response_id in OpenAiResponsesOptions.
- Also keeps an in-process session_id -> previous_response_id cache and auto-injects previous_response_id on later requests.
- Emits roci_session_id in metadata, but does not emit prompt_cache_key.
- Codex header builder does not currently add session_id header.

## Target behavior
- Prefer pi-mono-style semantics: full transcript in input + session-based cache hints.
- Keep requests self-contained by default; do not hide lineage in an in-process previous_response_id cache.
- Preserve explicit previous_response_id only as an advanced opt-in escape hatch if still needed.
- Emit session-based caching hints for OpenAI Responses/Codex where supported.

## Concrete patch list
1. Update OpenAI Responses adapter in crates/roci-providers/src/provider/openai_responses.rs
   - stop auto-reading previous_response_id from the in-process session cache by default
   - stop auto-writing response ids into the in-process session cache by default
   - continue honoring explicit OpenAiResponsesOptions.previous_response_id if caller sets it
   - add prompt_cache_key derived from request.session_id on the standard OpenAI Responses request body
   - add prompt_cache_key on Codex request body as well
   - add Codex session header(s) if appropriate for pi-mono alignment, starting with session_id
   - keep store behavior explicit/documented; do not widen persistence accidentally
2. Update request/config docs surface
   - document that session_id is for cache/session affinity on OpenAI Responses/Codex
   - document that previous_response_id is opt-in advanced behavior
3. Update tests
   - remove/replace tests that assert automatic session cache -> previous_response_id reuse
   - add tests asserting prompt_cache_key emission from session_id
   - add Codex header tests for session_id semantics
   - retain explicit previous_response_id test coverage

## Candidate files to patch
- crates/roci-providers/src/provider/openai_responses.rs
- crates/roci-core/src/types/generation.rs
- docs/architecture and/or docs/learned notes if behavior is user-facing
- relevant tests in crates/roci-providers/src/provider/openai_responses.rs

## Acceptance
- Default OpenAI Responses flow no longer depends on hidden in-process previous_response_id reuse.
- session_id drives request-level cache/session hints instead.
- explicit previous_response_id remains available only when deliberately requested.
- tests cover default behavior and explicit override behavior.
