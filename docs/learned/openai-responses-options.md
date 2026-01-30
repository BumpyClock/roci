# OpenAI Responses options (2026-01-30)

Sources:
- https://platform.openai.com/docs/api-reference/responses/create

Findings:
- `parallel_tool_calls` boolean (default true) controls whether tool calls may run in parallel.
- `previous_response_id` strings the request to the prior response; cannot be used with `conversation`.
- `instructions` is a system/developer message inserted for the request; using it with `previous_response_id` does not carry prior instructions forward.
- `metadata` is a map of up to 16 key-value pairs (string keys and values) for tagging responses.
- `service_tier` accepts `auto|default|flex|priority` to control processing tier.
- `truncation` appears in response objects and can be set on requests; examples show values like `disabled`.
