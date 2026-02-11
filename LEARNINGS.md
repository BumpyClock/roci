# Learnings

## Crate setup

- Renamed from TachikomaError → RociError for crate naming consistency.
- Directory-based modules (`mod.rs` pattern) to support multi-file modules.
- `thiserror` v2 requires edition 2021+; `rust-version = "1.75"` satisfies.
- `reqwest` with `default-features = false` + `rustls-tls` avoids native OpenSSL dep.
- Feature flags gate optional provider modules and capabilities (agent, audio, mcp, cli).
- `bon` v3 for builder pattern — replaces manual builders.
- `strum` 0.27 for enum Display/FromStr derivation.

## OpenAI provider

### API routing
- GPT-4.1 family uses Chat Completions; Responses API is reserved for o3/o4 + GPT-5.
- `GenerationSettings` includes Responses-specific options: parallel tool calls, previous response id, truncation, service tier, store, metadata.

### GPT-5 quirks
- Sampling params (`temperature`, `top_p`) only valid for gpt-5.2 with `reasoning_effort = none`; other GPT-5 models reject them.
- Chat requests use `max_completion_tokens` and drop sampling/penalty params for gpt-5 IDs.
- Responses requests default reasoning effort to medium and text verbosity to high.
- Stream text may arrive via `response.output_item.added` message content, not just `output_text` deltas.
- `text.verbosity` supported via `GenerationSettings.text_verbosity`.

### O3/O4
- Responses requests default truncation to auto.

### Parsing
- Responses parsing handles tool_call content and choices fallback; tool schemas normalize `additionalProperties` + `required`.

## Anthropic provider

### Extended thinking
- `ThinkingMode::Enabled { budget_tokens }` in `AnthropicOptions`.
- Temperature must NOT be sent when thinking is enabled (API rejects it).
- `max_tokens` must be ≥ `budget_tokens + 4096` and ≥ 16,384 when thinking is on.
- Beta headers (`interleaved-thinking-2025-05-14`, `fine-grained-tool-streaming-2025-05-14`) are always sent.
- Thinking blocks use `thinking` + `signature` fields; redacted blocks use `data` + `signature`.

### Streaming
- Handles `content_block_start` (tracks block type), `content_block_delta` (text/thinking/signature/input_json), `content_block_stop` (emits tool calls).

### Tool choice
- `Required` → `"any"`, `Function(name)` → `{"type": "tool", "name": name}`.
- `ToolChoice` is wired into `GenerationSettings` and the Anthropic provider (Tachikoma only had it for Realtime API).

### Types
- `ProviderResponse` carries `thinking: Vec<ContentPart>` for thinking blocks.
- `TextStreamDelta` carries `reasoning`, `reasoning_signature`, `reasoning_type` optional fields.
- `ContentPart` has `Thinking` and `RedactedThinking` variants.
- ANTHROPIC_API_KEY needed for live tests; add to `.env`.

## Google / Gemini provider

### Thinking config
- `GoogleOptions` in `GenerationSettings` with `thinking_config` and `safety_settings`.
- `GoogleThinkingConfig` has `budget_tokens` (Gemini 2.5), `include_thoughts`, `thinking_level` (Gemini 3).
- `GoogleThinkingLevel`: Minimal/Low/Medium/High serialized as SCREAMING_SNAKE_CASE.
- Serialized inside `generationConfig.thinkingConfig`; Roci fully wires it (Tachikoma did not).

### Safety settings
- `GoogleSafetyLevel`: Strict/Moderate/Relaxed mapped to Gemini threshold strings.
- Serialized as top-level `safetySettings` array.

### Structured output & tools
- Structured output uses `responseJsonSchema` in `generationConfig`.
- Function calls may include `thoughtSignature`; preserve it on tool call round-trips.
- Tool responses use role `"tool"` with `functionResponse` parts.

## Auth

- `roci::auth` module with token store, device-code session types, and provider helpers for OpenAI Codex, GitHub Copilot, and Claude Code imports.
- File-backed token storage uses TOML and `directories` for cross-platform home resolution.

## Live tests

- Coverage includes tool-call flows, JSON schema, streaming, and vision checks across OpenAI, Gemini, and Anthropic.
- Expanded for Gemini 3 Pro preview and OpenAI-compatible structured/streaming.
