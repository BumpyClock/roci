# Roci Architecture

Roci is a Rust AI SDK providing a unified interface for multiple AI providers,
with support for text generation, streaming, structured output, tool calling,
agents, audio, and MCP.

> **ADR**: See [docs/architecture/cli-soc.md](architecture/cli-soc.md) for the
> separation-of-concerns decision record.

## Workspace Structure

```
roci/                          # Cargo workspace root
├── src/                       # roci (core SDK crate)
├── crates/
│   ├── roci-cli/              # CLI binary: roci-agent
│   └── roci-tools/            # Built-in coding tools
├── tests/                     # Integration tests for core
├── examples/                  # Usage examples (require feature gates)
└── docs/
    ├── architecture/          # ADRs
    └── learned/               # Durable learnings
```

### `roci` -- Core SDK

Pure library crate. No `clap`, no terminal I/O, no `process::exit`.

| Module | Purpose |
|--------|---------|
| `auth` | Auth primitives: `AuthService`, `Token`, `FileTokenStore`, device-code flows, provider-specific auth (Claude Code, GitHub Copilot, OpenAI Codex) |
| `config` | `RociConfig` -- API keys, base URLs, token-store fallback |
| `error` | `RociError` with typed variants (`MissingCredential`, `MissingConfiguration`, `Api`, `Network`, etc.), categories, retryability |
| `generation` | `generate()`, `stream_text()`, `generate_object()` -- high-level generation API |
| `models` | `LanguageModel`, `ModelSelector`, `ProviderKey`, model capabilities and metadata |
| `provider` | Provider trait + implementations (see [Providers](#providers)) |
| `tools` | Tool trait, `ToolDefinition`, `ToolArguments`, validation -- traits only, no built-in implementations |
| `types` | Shared types: `ModelMessage`, `Usage`, `FinishReason`, `GenerationSettings`, `StreamEvent` |
| `stop` | Stop conditions: string match, regex, token count, timeout, predicate |
| `stream_transform` | Stream transforms: map, filter, buffer, throttle |
| `util` | `ResponseCache`, `UsageTracker`, `RetryPolicy` |
| `prelude` | Convenience re-exports |
| `agent` | Agent struct and configuration (feature: `agent`) |
| `agent_loop` | Agent execution loop with tool dispatch (feature: `agent`) |
| `audio` | Realtime audio sessions via WebSocket (feature: `audio`) |
| `mcp` | MCP client/server transport, multi-server aggregation (feature: `mcp`) |

**Feature gates**: `openai`, `anthropic`, `google`, `grok`, `groq`, `mistral`,
`ollama`, `lmstudio`, `azure`, `openrouter`, `together`, `replicate`,
`openai-compatible`, `anthropic-compatible`, `agent`, `audio`, `mcp`,
`all-providers`, `full`.

Default features: `openai`, `anthropic`, `google`.

### `roci-cli` -- CLI Binary

Produces the `roci-agent` binary. Owns all terminal concerns:

- `clap` argument parsing
- stdout/stderr output, spinners, interactive prompts
- Exit codes and `process::exit`
- User-facing error messages (maps core typed errors to help text)
- Auth flow orchestration (maps `AuthStep`/`AuthPollResult` to interactive prompts)

**Dependencies**: `roci` (with `agent` feature), `roci-tools`, `clap`, `tokio`, `chrono`.

### `roci-tools` -- Built-in Coding Tools

Standalone crate for agent coding tools. Import path: `roci_tools::builtin`.

| Tool | Description |
|------|-------------|
| `shell` | Execute shell commands with timeout |
| `read_file` | Read file contents (with truncation) |
| `write_file` | Write/create files (creates parent dirs) |
| `list_directory` | List directory entries with metadata |
| `grep` | Search file contents with regex |

**Usage**: `roci_tools::builtin::all_tools()` returns `Vec<AgentTool>`.

## Providers

All providers implement the `ModelProvider` trait behind feature gates.

| Provider | Module | Feature | Notes |
|----------|--------|---------|-------|
| OpenAI | `provider::openai` | `openai` | Chat Completions API |
| OpenAI Responses | `provider::openai_responses` | `openai` | Responses API for GPT-5/o4 reasoning models |
| Anthropic | `provider::anthropic` | `anthropic` | Claude API, extended thinking |
| Google | `provider::google` | `google` | Gemini API, thinking config |
| Grok | `provider::grok` | `grok` | OpenAI-compatible |
| Groq | `provider::groq` | `groq` | OpenAI-compatible |
| Mistral | `provider::mistral` | `mistral` | OpenAI-compatible |
| Ollama | `provider::ollama` | `ollama` | Local inference |
| LM Studio | `provider::lmstudio` | `lmstudio` | Local inference |
| Azure OpenAI | `provider::azure` | `azure` | Azure-hosted OpenAI |
| OpenRouter | `provider::openrouter` | `openrouter` | Multi-model router |
| Together | `provider::together` | `together` | OpenAI-compatible |
| Replicate | `provider::replicate` | `replicate` | Replicate API |
| GitHub Copilot | `provider::github_copilot` | `openai` | Device-code auth |
| OpenAI-compatible | `provider::openai_compatible` | `openai-compatible` | Generic OpenAI-compatible endpoint |
| Anthropic-compatible | `provider::anthropic_compatible` | `anthropic-compatible` | Generic Anthropic-compatible endpoint |

Shared utilities: `provider::format` (message formatting), `provider::http`
(HTTP client), `provider::sanitize` (message sanitization),
`provider::schema` (JSON Schema normalization).

## Import Paths

```rust
// Core SDK
use roci::prelude::*;
use roci::models::LanguageModel;
use roci::generation::generate;
use roci::auth::AuthService;
use roci::error::RociError;
use roci::types::{ModelMessage, GenerationSettings};

// Built-in tools
use roci_tools::builtin::all_tools;
```

## Error Strategy

Core errors use typed variants with metadata:

- `RociError::MissingCredential { provider }` -- no API key found
- `RociError::MissingConfiguration { key, provider }` -- missing config value
- `RociError::Api { status, message, .. }` -- provider API error

Each variant has a `category()` (Authentication, Configuration, Network, etc.)
and `is_retryable()` flag. The CLI crate maps these to user-facing messages
with actionable guidance.

## Testing

```bash
cargo test -p roci          # Core SDK (110+ unit tests, integration tests)
cargo test -p roci-cli      # CLI tests (arg parsing, error formatting)
cargo test -p roci-tools    # Tool tests (25 tests covering all tools)

# Live provider tests (requires API keys, --ignored)
cargo test --test live_providers -- --ignored --nocapture
```
