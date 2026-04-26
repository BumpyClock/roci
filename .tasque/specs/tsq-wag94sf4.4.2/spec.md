## Scope
Update request body/header construction in crates/roci-providers/src/provider/openai_responses.rs.

## Acceptance
- prompt_cache_key emitted from request.session_id for standard OpenAI Responses requests
- prompt_cache_key emitted for Codex requests
- Codex session header semantics evaluated/implemented starting with session_id
- regression tests added
