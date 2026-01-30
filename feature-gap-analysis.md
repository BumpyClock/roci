# Feature Gap Analysis: Roci (Rust) vs Tachikoma (Swift)

> Generated 2026-01-30 by parallel codebase analysis (6 focused agents).
> Tachikoma: `~/Projects/references/Tachikoma` | Roci: `~/Projects/roci`

---

## Executive Summary

Roci is a functional text-generation SDK with basic tool support. Tachikoma is a comprehensive AI platform with audio, realtime, MCP, embeddings, agent management, and deep provider-specific features. The gap is substantial across advanced features, streaming fidelity, and provider-specific options.

**Roci strengths**: formal `ModelProvider` trait, built-in retry with exponential backoff, cache/reasoning token tracking in `Usage`, per-model capability flags (`supports_json_schema`, `supports_reasoning`, `supports_system_messages`), explicit `max_output_tokens`, OpenAI Responses API options (`service_tier`, `truncation`, `store`).

**Tachikoma strengths**: full MCP protocol, realtime audio, embeddings, extended thinking, prompt caching control, multi-step agent loop, session persistence, rich provider-specific options (6 providers), comprehensive streaming events, OAuth auth.

---

## 1. Provider Coverage

| Provider | Tachikoma | Roci | Gap |
|----------|-----------|------|-----|
| OpenAI (Chat Completions) | Yes | Yes | Parity |
| OpenAI (Responses API) | Yes | Yes | Parity |
| Anthropic (Messages API) | Yes | Yes | Feature gaps below |
| Google (Gemini) | Yes | Yes | Parity |
| Grok / xAI | Yes | Yes | Roci missing vision model variants |
| Groq | Yes | Yes | Roci: fewer model defs (3 vs 6) |
| Mistral | Yes | Yes | Parity |
| Ollama | Yes (native `/api/chat`) | Yes (OpenAI-compat wrapper) | Tachikoma: health checks, image parsing from content, tool calls from JSON content |
| LMStudio | Yes (actor w/ health check) | Yes (OpenAI-compat wrapper) | Tachikoma: health check, model detection, 5min timeout |
| Azure OpenAI | Yes (full) | Yes (basic) | Tachikoma: richer endpoint/resource/version resolution chain |
| OpenRouter | Yes | Yes | Tachikoma: custom `HTTP-Referer`, `X-Title` headers |
| Together AI | Yes | Yes | Parity |
| Replicate | Yes (OpenAI-compat) | Stub (unimplemented) | **Gap: Roci stub only** |
| Anthropic-compatible | Yes | Yes (delegates) | Parity |
| OpenAI-compatible | Yes | Yes | Parity |
| DeepSeek | Models defined (maps to Ollama) | No model entries | Minor gap |

---

## 2. Model Definitions

### Missing Models in Roci

| Provider | Missing from Roci |
|----------|-------------------|
| Anthropic | `claude-opus-4-1` (opus4), `opus4Thinking`, `sonnet4Thinking`, `sonnet45` (newer model ID) |
| Grok | `grok4FastReasoning`, `grok4FastNonReasoning`, `grokCodeFast1`, `grok2Vision`, `grok2Image`, `grokVisionBeta`, `grokBeta` (11 models vs 5) |
| Groq | `llama370b`, `llama38b`, `gemma29b` (6 models vs 3) |
| Ollama | `gptOSS120B/20B`, `llava`, `bakllava`, `llama32Vision*`, `qwen25vl*`, `devstral`, `firefunction`, `commandR*`, `neuralChat`, ~20 more (31 vs 6) |
| LMStudio | `gptOSS*`, `llama370B/333B`, `mixtral8x7B`, `codeLlama34B`, `mistral7B`, `phi3Mini`, `current` (9 vs 1) |
| Mistral | `nemo`, `codestral` specific variant IDs (6 vs 5) |

### Context Length Discrepancies

| Model Family | Tachikoma | Roci | Action |
|-------------|-----------|------|--------|
| Claude (all) | 500,000 | 200,000 | **Fix: update to 500k** |
| GPT-5.1/5.2 | 400,000 | Not defined | Add context lengths |
| Gemini 2.5 Pro | 1,040,000 | 1,000,000 | Minor difference |

### Missing Capability Flags in Roci

| Flag | Tachikoma | Roci |
|------|-----------|------|
| `supports_audio_input` | Per-model | Not defined |
| `supports_audio_output` | Per-model | Not defined |
| `supports_realtime` | Per-model | Not defined |
| Heuristic vision detection (Ollama custom models) | Yes | No |

---

## 3. Streaming

### Critical Gaps

| Feature | Tachikoma | Roci |
|---------|-----------|------|
| **Anthropic event coverage** | `message_start`, `content_block_start`, `content_block_delta` (text + thinking + signature + tool JSON), `content_block_stop`, `message_delta`, `message_stop` | Only `content_block_delta` (text only), `message_stop`, `message_delta` |
| **Thinking/reasoning deltas** | Full: `thinking_delta`, `signature_delta`, `redacted_thinking` | None |
| **Tool call streaming** | Partial JSON accumulation from `input_json_delta` | None |
| **Response channels** | `.thinking`, `.analysis`, `.commentary`, `.final` | None |
| **Reasoning metadata** | `reasoningSignature`, `reasoningType` per delta | None |
| **Stream transforms** | Filter, map, buffer, throttle pipeline | None |
| **Usage mid-stream** | Supported | Only at stream end |
| **Buffered emission** | 20-char text buffering for efficiency | Immediate yield |

### Streaming Event Types

| Event Type | Tachikoma | Roci |
|-----------|-----------|------|
| `textDelta` | Yes | Yes |
| `toolCall` | Yes | Yes (ToolCallDelta) |
| `toolResult` | Yes | No |
| `reasoning` | Yes | Yes (thinking_delta, signature_delta) |
| `done` | Yes | Yes |
| `error` | Structured propagation | Basic |
| `start` | Yes | Yes |

---

## 4. Tool Calling

| Feature | Tachikoma | Roci |
|---------|-----------|------|
| Tool definitions | Full schema + typed parameters + `ParameterDefinition` enum | JSON Schema via serde_json |
| **Tool choice** (`auto`/`required`/`none`/specific) | Yes (`OpenAIOptions.toolChoice`) | Implemented in Anthropic; Tachikoma text providers also unimplemented |
| Parallel tool calls | `OpenAIOptions.parallelToolCalls` | Only `OpenAiResponsesOptions` |
| Tool result handling | Automatic loop with context | Manual or `stream_text_with_tools` |
| Tool execution context | Full: messages, model, settings, sessionId, stepIndex, metadata | Not provided to tools |
| Dynamic tool discovery | `DynamicToolProvider` protocol | Not implemented |
| Namespace/multi-agent routing | `AgentToolCall.namespace` | Only `recipient` field |
| Typed arguments | `AnyAgentToolValue` (runtime type info) | `serde_json::Value` |
| Streaming tool calls | Partial JSON accumulation from deltas | Anthropic: `input_json_delta` accumulation + `content_block_stop` emit |
| Max tool iterations | Configurable (default 5) | Fixed at 20 |
| Schema normalization | Broader framework | OpenAI: adds `additionalProperties: false`; Google: strips it |

---

## 5. Anthropic-Specific Features

| Feature | Tachikoma | Roci |
|---------|-----------|------|
| **Extended thinking** | Full: `ThinkingMode` (enabled/disabled), `budgetTokens`, thinking + redacted_thinking content blocks | Full: `ThinkingMode` enum, `AnthropicOptions.thinking`, thinking/redacted content blocks, streaming deltas |
| **Prompt caching** | `CacheControl` enum (ephemeral/persistent) in `AnthropicOptions` | Only tracks cache tokens in `Usage` response |
| **Beta headers** | Configurable with flag merging (`interleaved-thinking-2025-05-14`, `fine-grained-tool-streaming-2025-05-14`) | Always sent via `anthropic_headers()` with beta parameter |
| **Auth flexibility** | API key + Bearer token fallback chain | API key only |
| **Error parsing** | `AnthropicErrorResponse` structure (type + message extraction) | Generic `status_to_error()` |
| **top_k** parameter | Yes | Sent to API |
| **metadata** in request | Yes | Not sent |
| **Debug mode** | `DEBUG_ANTHROPIC` env var | Not supported |
| **Thinking fallback** | Graceful fallback from thinking mode on failure | N/A |

---

## 6. Provider-Specific Options

### Tachikoma Has (Roci Missing)

| Provider | Option | Impact |
|----------|--------|--------|
| **Anthropic** | thinking mode (enabled/disabled + budget) | ✅ Implemented |
| **Anthropic** | cache_control (ephemeral/persistent) | ✅ Type defined (matches Tachikoma) |
| **Anthropic** | metadata | Request metadata |
| **Google** | thinkingConfig (budgetTokens + includeThoughts) | ✅ Implemented |
| **Google** | safetySettings (strict/moderate/relaxed) | ✅ Implemented |
| **Mistral** | safeMode | Safety mode |
| **Groq** | speed (normal/fast/ultraFast) | Inference speed |
| **Grok** | funMode, includeCurrentEvents | Provider-specific |
| **OpenAI** | logprobs, topLogprobs, n | Response variants |

### Roci Has (Tachikoma Missing)

| Option | Notes |
|--------|-------|
| OpenAI Responses: `service_tier` (Auto/Default/Flex/Priority) | Request priority |
| OpenAI Responses: `truncation` (Auto/Disabled) | Context management |
| OpenAI Responses: `store` | Response caching |
| OpenAI Responses: `instructions` | System-like instructions |
| `text_verbosity` as top-level setting | GPT-5 verbosity |

---

## 7. Generation Settings

| Setting | Tachikoma | Roci |
|---------|-----------|------|
| max_tokens | Yes | Yes |
| temperature | Yes | Yes |
| top_p | Yes | Yes |
| top_k | Yes | Yes |
| frequency_penalty | Yes | Yes |
| presence_penalty | Yes | Yes |
| stop_sequences | Yes | Yes |
| seed | Yes | Yes |
| reasoning_effort | Yes (low/medium/high) | Yes (none/low/medium/high) |
| text_verbosity | Nested in OpenAIOptions.verbosity | Top-level |
| response_format | Nested in OpenAIOptions | Top-level |
| **stop_conditions** (programmatic) | Yes (function type) | No |
| **user** identifier | Not visible | Yes |
| **openai_responses** (nested options) | No | Yes |
| **provider_options** (per-provider) | Yes (6 providers) | No |

---

## 8. Agent / Multi-Turn

| Feature | Tachikoma | Roci |
|---------|-----------|------|
| Agent class | Full `Agent<Context>` with session management | Basic `Agent` struct |
| Multi-step tool loop | Yes (configurable `maxSteps`, default 5) | `stream_text_with_tools` (max 20, fixed) |
| Session persistence | JSON file storage with lifecycle management | In-memory only |
| Session metadata | createdAt, lastAccessedAt, messageCount | None |
| Conversation tracking | `Conversation` class with full history + branching | `Conversation` struct (messages only) |
| System prompt updates | Mid-session updates supported | Static only |
| Step tracking | `GenerationStep` (per-step text, toolCalls, toolResults, usage, finishReason) | Not tracked |
| Agent response | text, usage, finishReason, steps, conversationLength | Basic text + usage |
| Conversation builder | Fluent `.system()/.user()/.assistant()` | Manual `ModelMessage::system()` etc. |

---

## 9. MCP (Model Context Protocol)

| Feature | Tachikoma | Roci |
|---------|-----------|------|
| **Status** | Full implementation | **Stub** (all methods throw `UnsupportedOperation`) |
| Transports | Stdio, SSE, HTTP | Trait architecture only, no concrete implementations |
| Tool discovery | `tools/list` | Not implemented |
| Tool execution | `tools/call` | Not implemented |
| Protocol version fallback | 2025-03-26, 2024-11-05 | None |
| Auto-reconnect | Yes | No |
| Server config | Timeouts, headers, env vars | None |
| Multi-server aggregation | Yes | No |

---

## 10. Audio & Realtime

| Feature | Tachikoma | Roci |
|---------|-----------|------|
| Transcription | Full API (language hints, timestamps, formats) | `AudioProvider` trait stub only |
| TTS | Voice selection, speed, format options | `SpeechProvider` trait stub only |
| Realtime WebSocket | Full `RealtimeSession`: state management, heartbeat, event streaming, tool execution | `connect()` throws "not yet implemented" |
| Recording | Native audio recording module | None |
| Audio content parts | Yes (per-model input/output flags) | Not defined in `ModelCapabilities` |
| Models: OpenAI audio input | GPT-5+, GPT-4o | Not tracked |
| Models: Gemini audio input | Gemini 2.5+, Gemini 3 | Not tracked |

---

## 11. Embeddings

| Feature | Tachikoma | Roci |
|---------|-----------|------|
| **Status** | Full implementation | **Not implemented** |
| API | `generateEmbedding()`, `generateEmbeddingsBatch()` | None |
| Models | OpenAI (ada, 3-small, 3-large), Cohere, Voyage, custom | None |
| Batch processing | Concurrency control (default 5) | None |

---

## 12. Authentication

| Feature | Tachikoma | Roci |
|---------|-----------|------|
| Environment variables | Yes | Yes |
| `.env` file loading | Not visible | Yes (`dotenvy`) |
| Secure credential file | `~/.peekaboo/credentials` (0600 perms) | None |
| OAuth flow | OpenAI, Anthropic (browser + `--no-browser`) | None |
| Credential validation | With timeout | None |
| Auth fallback chain | config > auth manager > env | code > env |
| Provider normalization | "xai" → grok, etc. | Direct mapping |

---

## 13. Error Handling

| Feature | Tachikoma | Roci |
|---------|-----------|------|
| Error system | `TachikomaError` + `APICallError` + `TachikomaUnifiedError` | Single `RociError` enum |
| Recovery suggestions | Typed actions + helpURL + metadata | Enum-based suggestions |
| **Retry logic** | Not built-in | **Built-in** (exponential backoff + jitter, configurable) |
| Provider-specific parsing | Anthropic + OpenAI-specific response parsing | Generic `status_to_error()` |
| Finish reasons | stop, length, toolCalls, contentFilter, error, **cancelled**, **other** | stop, length, toolCalls, contentFilter, error |
| Error categories | validation, authentication, rateLimit, model, network, tool, parsing, internal | Similar set |

---

## 14. Usage & Cost Tracking

| Feature | Tachikoma | Roci |
|---------|-----------|------|
| input_tokens | Yes | Yes |
| output_tokens | Yes | Yes |
| total_tokens | Yes | Yes |
| **cache_read_tokens** | No | **Yes** |
| **cache_creation_tokens** | No | **Yes** |
| **reasoning_tokens** | No | **Yes** |
| Cost calculation | Inline in Usage | Separate `Cost` struct with `from_usage()` |

---

## 15. Message & Content Types

| Feature | Tachikoma | Roci |
|---------|-----------|------|
| Roles | system, user, assistant, tool | System, User, Assistant, Tool |
| ContentPart: text | Yes | Yes |
| ContentPart: image | Yes | Yes |
| ContentPart: toolCall | Yes | Yes |
| ContentPart: toolResult | Yes | Yes |
| ContentPart: audio | Yes | **No** |
| ContentPart: file | Yes | **No** |
| ContentPart: refusal | Yes | **No** |
| ContentPart: reasoning | Yes | **No** |
| **ResponseChannel** (thinking/analysis/commentary/final) | Yes | **No** |
| Message metadata | Full (id, timestamp, channel, metadata) | Basic (role, content, name, timestamp) |
| Image input types | base64, url, filePath | Base64 {data, mime_type}, Url, Bytes {data, mime_type} |

---

## 16. CLI

| Feature | Tachikoma | Roci |
|---------|-----------|------|
| Executables | 3 (gpt5cli, ai-cli, TachikomaConfigCLI) | 1 (basic) |
| Auth management | OAuth + `config login/status/add/init` | None |
| Model selection | Rich | `--model` flag |
| Arguments | Full suite | `--model`, `--system`, `--temperature`, `--max-tokens`, `--stream` |

---

## 17. Testing

| Feature | Tachikoma | Roci |
|---------|-----------|------|
| Test files | 66 Swift test files | 9 Rust test files (~1,766 lines) |
| Test types | Unit, integration, CLI, audio, auth, mocks, snapshots | Unit, integration (live_providers), wiremock |
| Snapshot testing | CLI output snapshots | None |
| Test resources | Config fixtures | None |

---

## Priority Gaps for Roci

### P0 — Critical (blocks core use-case parity)

1. **Tool choice support** (`auto`/`required`/`none`/specific) — affects all providers — **Closed** (Tachikoma also unimplemented for text providers; only defined in Realtime API)

2. **Anthropic extended thinking** — thinking mode, budget tokens, thinking/redacted content blocks — **Complete**
3. **Anthropic streaming fidelity** — `content_block_start/stop`, `thinking_delta`, `signature_delta`, `input_json_delta` (tool streaming) — **Complete**
4. **Context length corrections** — Anthropic at 200k should be 500k+ — **Complete** (already fixed)
5. **Provider-specific options architecture** — extensible per-provider settings container — **Complete** (`AnthropicOptions` in `GenerationSettings`)

### P1 — High (significant functionality gaps)

6. **MCP implementation** — full protocol (Stdio + SSE transports minimum)
7. **Multi-step agent loop** — automatic tool execution with configurable max steps & step tracking
8. **Prompt caching control** — Anthropic `cache_control` headers in requests — **Closed** (Tachikoma defines CacheControl enum but doesn't wire to API; Roci has matching CacheControl in AnthropicOptions)
9. **Google thinking config** — Gemini 2.5+ thinking budget + includeThoughts — **Complete** (`GoogleOptions`, `GoogleThinkingConfig`, `GoogleThinkingLevel`, `GoogleSafetyLevel` types; serialized into `generationConfig.thinkingConfig` and top-level `safetySettings`)
10. **Beta header support** — Anthropic `interleaved-thinking`, `fine-grained-tool-streaming` — **Complete**
11. **Missing content parts** — audio, file, refusal, reasoning in `ContentPart` enum — Partially addressed (thinking/redacted_thinking added; audio/file/refusal still missing)
12. **Response channels** — thinking vs final output differentiation

### P2 — Medium (feature completeness)

13. **Embeddings API** — at least OpenAI models
14. **Session persistence** — save/restore conversation state to disk
15. **Realtime audio** — WebSocket `RealtimeSession` with event streaming
16. **Stream transforms** — filter, map, buffer, throttle pipeline
17. **Missing model definitions** — Grok vision variants, expanded Ollama/LMStudio/Groq catalogs
18. **Audio capability flags** — `supports_audio_input`/`supports_audio_output`
19. **Dynamic tool discovery** — `DynamicToolProvider` equivalent
20. **Streaming tool call accumulation** — partial JSON from deltas — Partially **Complete** (Anthropic: input_json_delta; OpenAI/Gemini: not yet)

### P3 — Low (nice-to-have)

21. **OAuth flow** — browser-based auth for OpenAI/Anthropic
22. **Secure credential storage** — file-based credstore
23. **CLI auth/config commands** — `config add/login/status`
24. **Stop conditions** — programmatic stop functions
25. **Health checks** — LMStudio/Ollama health endpoint checking
26. **Snapshot testing** — CLI output validation
27. **Reasoning signature tracking** — signature + reasoning type per stream delta — **Complete**
28. **Finish reason variants** — add `Cancelled`, `Other`
29. **Anthropic error parsing** — dedicated `AnthropicErrorResponse` structure
30. **Conversation builder** — fluent API for message construction
