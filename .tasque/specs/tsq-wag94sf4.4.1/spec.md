## Scope
Update crates/roci-providers/src/provider/openai_responses.rs so default behavior no longer auto-reads or auto-writes previous_response_id via the in-process session cache.

## Acceptance
- automatic session cache based previous_response_id reuse removed or disabled by default
- explicit OpenAiResponsesOptions.previous_response_id still works
- tests updated accordingly
