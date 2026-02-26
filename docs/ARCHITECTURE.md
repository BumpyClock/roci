# Roci Architecture

Roci is a Rust AI SDK providing a unified interface for multiple AI providers,
with support for text generation, streaming, structured output, tool calling,
agents, audio, and MCP.

> **ADRs**:
> - [docs/architecture/cli-soc.md](architecture/cli-soc.md) -- CLI/core SoC
> - [docs/architecture/providers-soc.md](architecture/providers-soc.md) -- Provider/core SoC

## Workspace Structure

```
roci/                          # Cargo workspace root
├── src/lib.rs                 # roci meta-crate (re-exports + default wiring)
├── crates/
│   ├── roci-core/             # Provider-agnostic SDK kernel
│   ├── roci-providers/        # Built-in provider transports + OAuth flows
│   ├── roci-cli/              # CLI binary: roci-agent
│   └── roci-tools/            # Built-in coding tools
├── tests/                     # Integration tests
├── examples/                  # Usage examples
└── docs/
    ├── architecture/          # ADRs
    └── learned/               # Durable learnings
```

## Crate Dependency Graph

```
  roci-cli ─────────┐
       │            │
       ▼            ▼
    roci-tools    roci  (meta-crate)
                   │  │
                   │  └──────────┐
                   ▼             ▼
            roci-providers   roci-core
                   │             ▲
                   └─────────────┘
```

- `roci-core` has zero provider dependencies. Third-party crates depend on it alone.
- `roci-providers` depends on `roci-core`; adds all built-in transports + OAuth.
- `roci` (meta-crate) re-exports both with default initialization.
- `roci-cli` and `roci-tools` depend on `roci`.

## Usage Paths

### Batteries-included (recommended)

Depend on `roci`. Use `default_registry()` and `default_auth_service()`:

```rust
use roci::prelude::*;

let config = RociConfig::from_env();
let registry = roci::default_registry();      // all enabled built-in providers
let provider = registry.create_provider("openai", "gpt-4o", &config)?;
let text = roci::generation::generate(provider.as_ref(), "Hello").await?;
```

### Explicit wiring (core-only)

Depend on `roci-core` + `roci-providers` directly. Wire your own registry:

```rust
use roci_core::prelude::*;

let mut registry = ProviderRegistry::new();
// Register only the providers you need, including custom ones
registry.register(Arc::new(MyCustomFactory));
```

See `examples/custom_provider.rs` for a complete working example.

## Crate Details

### `roci` -- Meta-crate

Thin re-export facade. Provides two convenience constructors:

| Function | Purpose |
|----------|---------|
| `default_registry()` | `ProviderRegistry` pre-loaded with all enabled built-in providers |
| `default_auth_service(store)` | `AuthService` pre-loaded with all built-in auth backends |

Re-exports `roci_core::*` so import paths like `roci::prelude::*`,
`roci::provider::ModelProvider`, `roci::models::LanguageModel` all work.

### `roci-core` -- Provider-agnostic SDK Kernel

Pure library crate. No provider implementations, no `clap`, no terminal I/O.

| Module | Purpose |
|--------|---------|
| `provider` | `ModelProvider` trait, `ProviderFactory` trait, `ProviderRegistry`, `ProviderRequest`/`ProviderResponse`, `ToolDefinition` |
| `provider::http` | `shared_client()`, `bearer_headers()`, `parse_sse_data()`, `status_to_error()` |
| `provider::format` | `tool_result_to_string()` |
| `provider::schema` | `normalize_schema_for_provider()` |
| `provider::sanitize` | `sanitize_messages_for_provider()` |
| `models` | `LanguageModel` (string-based), `ProviderKey`, `ModelSelector`, `ModelCapabilities` |
| `auth` | `AuthService` orchestrator, `AuthBackend` trait, `Token`, `FileTokenStore`, `DeviceCodeSession` |
| `config` | `RociConfig`, `AuthManager`, `AuthValue` |
| `error` | `RociError` with typed variants, categories, retryability |
| `types` | `ModelMessage`, `Usage`, `FinishReason`, `GenerationSettings`, `TextStreamDelta`, `ContentPart` |
| `generation` | `generate_text()`, `stream_text()`, `generate_object()` -- operate on `&dyn ModelProvider` |
| `skills` | Skill discovery, frontmatter parsing, and prompt formatting |
| `resource` | Resource loading for settings, context files, prompt templates, and diagnostics |
| `tools` | `Tool` trait, `AgentTool`, `ToolArguments`, `DynamicTool` |
| `stream_transform` | `StreamTransform` trait + built-in transforms |
| `stop` | Stop conditions |
| `util` | `ResponseCache`, `UsageTracker`, `RetryPolicy` |
| `prelude` | Convenience re-exports |
| `agent` / `agent_loop` | `AgentRuntime`, evented loop runner, approvals, and compaction/summary pipeline (feature: `agent`) |
| `audio` | Realtime audio sessions via WebSocket (feature: `audio`) |
| `mcp` | MCP client/server transport (feature: `mcp`) |

#### Agent runtime subsystem (`agent` feature)

- `AgentRuntime` is the high-level stateful API (prompt/continue/follow-up/steer/reset/abort, snapshots/watchers).
- `agent_loop::runner` executes provider turns, streaming, tool execution, approvals, retries, and event emission.
- Compaction is supported in two modes:
  - automatic pre-provider compaction in the run loop when reserved context budget would be exceeded
  - explicit/manual compaction via `AgentRuntime::compact()`
- Branch summaries are explicit-only via `AgentRuntime::summarize_branch_entries(...)` (not auto-triggered).
- Summary model selection follows settings fallback:
  - conversation compaction: `compaction.model` else current run model
  - branch summary: `branch_summary.model` else current run model
- Session hook interfaces are available in `AgentConfig`:
  - `session_before_compact` supports continue/cancel/override-summary and accepts a full compaction override object
  - `session_before_compact` payload now includes cancellation signal/token
  - `session_before_compact` cancel aborts manual compaction with an error; cancel from auto-compaction aborts the run
  - `session_before_tree` supports continue/cancel/override-summary; instruction/label overrides are deferred
- Tool lifecycle hook interfaces are available in `RunHooks` and surfaced through `AgentConfig`:
  - `pre_tool_use` supports continue/block/rewrite-args before tool execution
  - `post_tool_use` can transform tool results (including synthetic error results)
  - legacy `tool_result_persist` has been replaced by `post_tool_use`

### `roci-providers` -- Built-in Transports + OAuth

All concrete provider implementations and auth backends. Each provider is
behind a feature flag.

**Provider transports:**

| Provider | Module | Feature | Notes |
|----------|--------|---------|-------|
| OpenAI | `openai` | `openai` | Chat Completions API |
| OpenAI Responses | `openai_responses` | `openai` | Responses API for GPT-5/o4 |
| Anthropic | `anthropic` | `anthropic` | Claude API, extended thinking |
| Google | `google` | `google` | Gemini API, thinking config |
| Grok | `grok` | `grok` | OpenAI-compatible |
| Groq | `groq` | `groq` | OpenAI-compatible |
| Mistral | `mistral` | `mistral` | OpenAI-compatible |
| Ollama | `ollama` | `ollama` | Local inference |
| LM Studio | `lmstudio` | `lmstudio` | Local inference |
| Azure OpenAI | `azure` | `azure` | Azure-hosted OpenAI |
| OpenRouter | `openrouter` | `openrouter` | Multi-model router |
| Together | `together` | `together` | OpenAI-compatible |
| Replicate | `replicate` | `replicate` | Replicate API |
| GitHub Copilot | `github_copilot` | `openai` | Device-code auth |
| OpenAI-compatible | `openai_compatible` | `openai-compatible` | Generic endpoint |
| Anthropic-compatible | `anthropic_compatible` | `anthropic-compatible` | Generic endpoint |

**OAuth flows:** `ClaudeCodeAuth`, `GitHubCopilotAuth`, `OpenAiCodexAuth`.

**Registration functions:**
- `register_default_providers(registry)` -- registers a `ProviderFactory` for each enabled provider
- `register_default_auth_backends(service)` -- registers an `AuthBackend` for each OAuth provider

Provider-specific model enums (`OpenAiModel`, `AnthropicModel`, etc.) live in
`roci-providers` and are used internally. They do not appear in the core API.

### `roci-cli` -- CLI Binary

Produces the `roci-agent` binary. Owns all terminal concerns:

- command surface: `roci-agent auth ...` and `roci-agent chat ...`
- `clap` argument parsing
- stdout/stderr output, spinners, interactive prompts
- Exit codes and `process::exit`
- User-facing error messages (maps core typed errors to help text)
- Auth flow orchestration (maps `AuthStep`/`AuthPollResult` to interactive prompts)
- Resource diagnostics rendering (surfaces loader warnings from `roci-core::resource`)

Resource loading behavior used by CLI chat:
- Reads settings from `~/.roci/agent/settings.json` and `.roci/settings.json` (project overrides global).
- Discovers context files with per-directory precedence `AGENTS.md` > `CLAUDE.md`.
- Resolves system prompts from `SYSTEM.md` and `APPEND_SYSTEM.md` with project-over-global precedence.
- Expands slash prompt templates from `prompts/*.md` with argument substitution.
- Builds final system prompt as: CLI `--system` (or discovered `SYSTEM.md`) + discovered `APPEND_SYSTEM.md` + rendered project context section.
- Loads skills from roots in precedence order: `.roci/skills`, `.agents/skills`, `~/.roci/agent/skills`, `~/.agents/skills` (plus explicit paths/roots from CLI flags).

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

**Usage**: `roci_tools::builtin::all_tools()` returns `Vec<Arc<dyn Tool>>`.

## Extension Points

### Custom Providers

Implement `ModelProvider` + `ProviderFactory`, then register with a
`ProviderRegistry`. See `examples/custom_provider.rs`.

```rust
struct MyFactory;
impl ProviderFactory for MyFactory {
    fn provider_keys(&self) -> &[&str] { &["my-provider"] }
    fn create(&self, config: &RociConfig, key: &str, model_id: &str)
        -> Result<Box<dyn ModelProvider>, RociError> { /* ... */ }
    fn parse_model(&self, _key: &str, _id: &str)
        -> Option<Box<dyn Any + Send + Sync>> { None }
}

let mut registry = roci::default_registry();
registry.register(Arc::new(MyFactory));
```

### Custom Auth Backends

Implement `AuthBackend`, then register with an `AuthService`:

```rust
let mut svc = roci::default_auth_service(store);
svc.register_backend(Arc::new(MyAuthBackend));
```

### Agent Session Hooks (feature: `agent`)

Inject pre-summary behavior through `AgentConfig`:

- `session_before_compact`: inspect prepared compaction payload (messages, token counts, file ops, settings, cancellation token) and choose continue/cancel/override-summary via a full override object.
- `session_before_tree`: inspect prepared branch-summary payload and choose continue/cancel/override-summary. Instruction/label overrides returned here are deferred rather than applied to the active branch immediately.

This allows policy enforcement and custom summarization without forking core loop logic.

## Feature Flags

| Feature | Owned by | Effect |
|---------|----------|--------|
| `openai`, `anthropic`, `google`, ... | `roci-providers` | Gates provider transport compilation |
| `all-providers` | `roci-providers` | Enables all provider features |
| `agent`, `audio`, `mcp` | `roci-core` | Gates agent loop, audio, MCP modules |
| `full` | `roci` (meta-crate) | Enables `all-providers` + `agent` + `audio` + `mcp` |

Pass-through: `roci` features forward to `roci-providers` and `roci-core`.
`roci-core` has **no** provider feature flags -- it is always provider-agnostic.

Default features (via `roci`): `openai`, `anthropic`, `google`.

## Import Paths

```rust
// Via meta-crate (recommended)
use roci::prelude::*;
use roci::models::LanguageModel;
use roci::generation::generate;
use roci::auth::AuthService;
use roci::error::RociError;
use roci::types::{ModelMessage, GenerationSettings};

// Direct core access
use roci_core::prelude::*;
use roci_core::provider::{ModelProvider, ProviderFactory, ProviderRegistry};

// Provider-specific types (from roci-providers)
use roci_providers::openai::OpenAiProvider;
use roci_providers::auth::claude_code::ClaudeCodeAuth;

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
cargo test -p roci-core       # Core SDK kernel
cargo test -p roci-core --features agent  # Agent runtime/loop + compaction/summary
cargo test -p roci-providers  # Provider transports
cargo test -p roci            # Meta-crate integration tests
cargo test -p roci-cli        # CLI tests (arg parsing, error formatting)
cargo test -p roci-tools      # Built-in tool tests

# Live provider smoke tests (requires API keys, --ignored)
cargo test --test live_providers -- --ignored --nocapture
```
