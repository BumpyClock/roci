# OpenAI + Gemini API shapes (2026-01-30)

## OpenAI Responses API
- Function calls are returned in the response `output` array as items with `type: "function_call"` plus `call_id`, `name`, and JSON-encoded `arguments`. Tool outputs are sent back as `function_call_output` items keyed by the same `call_id`. Source: https://platform.openai.com/docs/guides/function-calling
- Streaming tool calls emit `response.output_item.added` followed by `response.function_call_arguments.delta` events and a final `response.function_call_arguments.done` event with the full arguments payload. Source: https://platform.openai.com/docs/guides/function-calling and https://platform.openai.com/docs/api-reference/responses-streaming/response/reasoning
- Structured outputs require `additionalProperties: false` on object schemas; JSON mode requires an explicit JSON instruction in context. Sources: https://platform.openai.com/docs/guides/structured-outputs and https://platform.openai.com/docs/guides/structured-outputs/additionalproperties-false-must-always-be-set-in-objects%3A

## Gemini (Google)
- Structured output uses `generationConfig.responseMimeType = "application/json"` and `responseJsonSchema` to constrain model output to a JSON Schema subset. Source: https://ai.google.dev/gemini-api/docs/structured-output
- Function calling uses `functionCall` parts in model output and `functionResponse` parts in tool responses; thought signatures must be preserved when the model includes them. Sources: https://ai.google.dev/gemini-api/docs/function-calling and https://ai.google.dev/gemini-api/docs/thought-signatures
