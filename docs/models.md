# Models

Roci ships with built-in model enums per provider plus custom model IDs.

## OpenAI (`LanguageModel::OpenAi`)

- gpt-4o, gpt-4o-mini, gpt-4-turbo, gpt-4, gpt-3.5-turbo
- gpt-4o-realtime-preview
- gpt-4.1, gpt-4.1-mini, gpt-4.1-nano
- o1, o1-mini, o1-pro, o3, o3-mini, o4-mini
- gpt-5, gpt-5.1, gpt-5.2, gpt-5-pro, gpt-5-mini, gpt-5-nano
- gpt-5-thinking, gpt-5-thinking-mini, gpt-5-thinking-nano
- gpt-5-chat-latest

Notes:
- GPT-5 family uses the Responses API. `GenerationSettings.max_tokens` maps to `max_output_tokens`.
- GPT-4.1 family uses the Chat Completions API.
- GPT-5 family does not accept `temperature` or `top_p`.
- GPT-5.2 accepts `temperature` or `top_p` only when `reasoning_effort = none`.
- GPT-5 models support `GenerationSettings.text_verbosity` with `low`, `medium`, or `high`.
- GPT-5 defaults to `text_verbosity = high` and `reasoning_effort = medium` when unset.
- Reasoning models (o3/o4) default to `truncation = auto` when unset.

## Google (`LanguageModel::Google`)

- gemini-3-flash, gemini-3-flash-preview, gemini-3-pro-preview
- gemini-2.5-pro, gemini-2.5-flash, gemini-2.5-flash-lite, gemini-2.0-flash
- gemini-1.5-pro, gemini-1.5-flash

Notes:
- `gemini-3-flash` currently uses the `gemini-3-flash-preview` API id.

## OpenAI compatible (`LanguageModel::OpenAiCompatible`)

- `OpenAiCompatibleModel { model_id, base_url }`
- Uses OpenAI-compatible Chat Completions API.

## Other providers

See enums under `src/models` for the current catalog.
