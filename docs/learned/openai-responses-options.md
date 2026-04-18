# OpenAI Responses options (2026-01-30)

Sources:
- https://platform.openai.com/docs/api-reference/responses/create

Findings:
- `parallel_tool_calls` boolean (default true) controls whether tool calls may run in parallel.
- `previous_response_id` strings the request to the prior response; OpenAI documents it as incompatible with `conversation`, though roci does not currently expose a `conversation` option here.
- `instructions` is a system/developer message inserted for the request; using it with `previous_response_id` does not carry prior instructions forward.
- `metadata` is a map of up to 16 key-value pairs (string keys and values) for tagging responses.
- `service_tier` accepts `auto|default|flex|priority` to control processing tier.
- `truncation` appears in response objects and can be set on requests; examples show values like `disabled`.

## Session semantics (2026-03-06)

Aligned with pi-mono's stateless approach:

- `session_id` on `ProviderRequest` drives `prompt_cache_key` in the request body for server-side prompt-cache affinity. This applies to both standard OpenAI Responses and Codex endpoints.
- When `session_id` is present, OpenAI Responses requests also send aligned cache-affinity headers (`session_id` and `x-client-request-id`). This keeps direct OpenAI, Codex, and OpenAI-compatible proxy behavior closer to pi-mono/Codex.
- `session_id` is not injected into request `metadata` by default.
- `previous_response_id` in `OpenAiResponsesOptions` is opt-in only — callers must set it explicitly. There is no automatic in-process caching of response IDs.
- Default flow sends the full transcript in `input` each request, relying on `prompt_cache_key` for efficient caching rather than server-side conversation state via `previous_response_id`.
