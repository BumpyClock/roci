# OpenAI + Gemini API shapes (2026-01-30)

## OpenAI Responses API
- Function calls are returned in the response `output` array as items with `type: "function_call"` plus `call_id`, `name`, and JSON-encoded `arguments`. Tool outputs are sent back as `function_call_output` items keyed by the same `call_id`. Source: https://platform.openai.com/docs/guides/function-calling
- Streaming tool calls emit `response.output_item.added` followed by `response.function_call_arguments.delta` events and a final `response.function_call_arguments.done` event with the full arguments payload. Source: https://platform.openai.com/docs/guides/function-calling and https://platform.openai.com/docs/api-reference/responses-streaming/response/reasoning
- Structured outputs require `additionalProperties: false` on object schemas; JSON mode requires an explicit JSON instruction in context. Sources: https://platform.openai.com/docs/guides/structured-outputs and https://platform.openai.com/docs/guides/structured-outputs/additionalproperties-false-must-always-be-set-in-objects%3A

## OpenAI-compatible Chat Completions
- Reasoning models may stream private reasoning in the vendor extensions `choices[].delta.reasoning_content`, `reasoning`, or `reasoning_text`, and answer text in `choices[].delta.content`. Use the first non-empty reasoning field to avoid duplicate aliases, normalize it to `StreamEventType::Reasoning`, and never append it to assistant text. Verified against LM Studio's OpenAI-compatible endpoint with `glm-4.7-flash-mlx`.

## Gemini (Google)
- Structured output uses `generationConfig.responseMimeType = "application/json"` and `responseJsonSchema` to constrain model output to a JSON Schema subset. Source: https://ai.google.dev/gemini-api/docs/structured-output
- Function calling uses `functionCall` parts in model output and `functionResponse` parts in tool responses; thought signatures must be preserved when the model includes them. Sources: https://ai.google.dev/gemini-api/docs/function-calling and https://ai.google.dev/gemini-api/docs/thought-signatures
