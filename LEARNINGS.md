# Learnings

## 2026-01-29: Initial scaffolding
- Renamed from TachikomaError → RociError for crate naming consistency.
- Directory-based modules (`mod.rs` pattern) to support multi-file modules.
- `thiserror` v2 requires edition 2021+; `rust-version = "1.75"` satisfies.
- `reqwest` with `default-features = false` + `rustls-tls` avoids native OpenSSL dep.
- Feature flags gate optional provider modules and capabilities (agent, audio, mcp, cli).
- `bon` v3 for builder pattern — replaces manual builders.
- `strum` 0.27 for enum Display/FromStr derivation.

## 2026-01-30: Provider parity notes
- GPT-5 sampling params only valid for gpt-5.2 with `reasoning_effort = none`; other GPT-5 models reject `temperature` and `top_p`.
- Gemini function calls may include `thoughtSignature`; preserve it on tool call round-trips.

## 2026-01-30: GPT-5 verbosity + Gemini tool role
- GPT-5 family supports Responses API `text.verbosity` via `GenerationSettings.text_verbosity`.
- Gemini tool responses should use role "tool" with `functionResponse` parts.

## 2026-01-30: Live tool coverage
- Live provider tests now include tool-call flows per provider.

## 2026-01-30: Live structured/stream/vision coverage
- Live provider tests now include JSON schema, streaming, and vision checks for OpenAI/Gemini.

## 2026-01-30: OpenAI/Gemini parity adjustments
- OpenAI Responses parsing now handles tool_call content and choices fallback; tool schemas normalize additionalProperties + required.
- Gemini structured output uses responseJsonSchema in generationConfig.
- GPT-4.1 family now uses Chat Completions provider; Responses API reserved for o3/o4 + GPT-5.

## 2026-01-30: Responses options + live test expansion
- GenerationSettings now includes OpenAI Responses options (parallel tool calls, previous response id, truncation, service tier, store, metadata).
- Live provider tests expanded for Gemini 3 Pro preview and OpenAI-compatible structured/streaming coverage.
- GPT-5 stream text may arrive via `response.output_item.added` message content, not just output_text deltas.

## 2026-01-30: OpenAI Responses defaults + GPT-5 compat
- GPT-5 Responses requests now default reasoning effort to medium and text verbosity to high.
- O3/O4 Responses requests default truncation to auto.
- OpenAI chat requests use max_completion_tokens and drop sampling/penalty params for gpt-5 IDs.

## 2026-01-30: Anthropic extended thinking + tool choice
- Extended thinking implemented: `ThinkingMode::Enabled { budget_tokens }` in `AnthropicOptions`.
- Temperature must NOT be sent when thinking is enabled (Anthropic API rejects it).
- `max_tokens` must be ≥ `budget_tokens + 4096` and ≥ 16,384 when thinking is on.
- Beta headers (`interleaved-thinking-2025-05-14`, `fine-grained-tool-streaming-2025-05-14`) are always sent.
- Thinking blocks use `thinking` + `signature` fields; redacted blocks use `data` + `signature`.
- Streaming handles `content_block_start` (tracks block type), `content_block_delta` (text/thinking/signature/input_json), `content_block_stop` (emits tool calls).
- Anthropic tool_choice maps: `Required` → `"any"`, `Function(name)` → `{"type": "tool", "name": name}`.
- Tachikoma's `ToolChoice` enum is defined but NOT wired to text providers (only Realtime API); Roci now has it in `GenerationSettings` and Anthropic provider.
- `ProviderResponse` now carries `thinking: Vec<ContentPart>` for thinking blocks.
- `TextStreamDelta` now carries `reasoning`, `reasoning_signature`, `reasoning_type` optional fields.
- `ContentPart` now has `Thinking` and `RedactedThinking` variants.
- ANTHROPIC_API_KEY needed for live tests; add to `.env`.
