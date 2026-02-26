# ADR: Provider / Core Separation of Concerns

## Status

Accepted

## Date

2026-02-25

## Context

The `roci` crate bundles provider-agnostic abstractions (traits, types, config, auth orchestration, agent loop) alongside 15+ concrete provider transports and 3 OAuth flow implementations. This creates:

- Compile-time bloat: consumers needing one provider must compile all feature-gated code.
- Coupling: adding a provider requires modifying core (`LanguageModel` enum, `create_provider()`, `ModelSelector`, feature flags).
- No extension point: third-party providers cannot register without forking core.

The CLI/core split (`cli-soc.md`) established workspace conventions. This ADR extends the pattern to separate provider-agnostic core from built-in provider implementations.

## Decision

Split the current root `roci` crate into three:

1. **`roci-core`** (`crates/roci-core/`) — provider-agnostic SDK kernel.
2. **`roci-providers`** (`crates/roci-providers/`) — built-in provider transports + OAuth flows.
3. **`roci`** (root `src/`) — thin meta-crate re-exporting both with default wiring.

### 1. Crate Layout

```
roci/                              # Cargo workspace root
├── src/lib.rs                     # roci meta-crate (re-exports + default init)
├── crates/
│   ├── roci-core/                 # Provider-agnostic SDK kernel
│   ├── roci-providers/            # Built-in transports + OAuth flows
│   ├── roci-cli/                  # CLI binary (unchanged)
│   └── roci-tools/                # Built-in coding tools (unchanged)
```

### 2. roci-core Owns

Everything provider-agnostic that any consumer or third-party provider needs:

| Module | Contents |
|--------|----------|
| `provider` | `ModelProvider` trait, `ProviderRequest`, `ProviderResponse`, `ToolDefinition`, `ProviderFactory` trait, `ProviderRegistry` |
| `provider::http` | `shared_client()`, `bearer_headers()`, `anthropic_headers()`, `parse_sse_data()`, `status_to_error()` |
| `provider::format` | `tool_result_to_string()` |
| `provider::schema` | `normalize_schema_for_provider()` |
| `provider::sanitize` | `sanitize_messages_for_provider()` |
| `models` | `LanguageModel` (simplified), `ProviderKey`, `ModelSelector`, `ModelCapabilities` |
| `auth` | `AuthService` (generic orchestrator), `AuthBackend` trait, `AuthStep`, `AuthPollResult`, `AuthError`, `Token`, `TokenStore`, `FileTokenStore`, `DeviceCodeSession`, `DeviceCodePoll` |
| `config` | `RociConfig`, `AuthManager`, `AuthValue` |
| `error` | `RociError`, `ErrorCategory`, `ErrorDetails`, `RecoverySuggestion` |
| `types` | `ModelMessage`, `Usage`, `FinishReason`, `GenerationSettings`, `TextStreamDelta`, `ContentPart`, `AgentToolCall`, `AgentToolResult`, `Role` |
| `generation` | `generate_text()`, `stream_text()`, `generate_object()`, `stream_object()` — operate on `&dyn ModelProvider` |
| `tools` | `Tool` trait, `AgentTool`, `ToolArguments`, `DynamicTool` |
| `stream_transform` | `StreamTransform` trait + built-in transforms |
| `stop` | Stop conditions |
| `util` | `ResponseCache`, `UsageTracker`, `RetryPolicy` |
| `agent` / `agent_loop` | `AgentRuntime`, evented runner, approvals, and compaction/summary pipeline (feature: `agent`) |
| `audio` | Realtime audio (feature: `audio`) |
| `mcp` | MCP transport (feature: `mcp`) |
| `prelude` | Convenience re-exports |

### 3. roci-providers Owns

All concrete provider implementations and OAuth flows:

**Provider transports** (each behind a feature flag):

| Module | Feature |
|--------|---------|
| `provider::openai`, `provider::openai_responses` | `openai` |
| `provider::anthropic` | `anthropic` |
| `provider::google` | `google` |
| `provider::grok` | `grok` |
| `provider::groq` | `groq` |
| `provider::mistral` | `mistral` |
| `provider::ollama` | `ollama` |
| `provider::lmstudio` | `lmstudio` |
| `provider::azure` | `azure` (depends on `openai`) |
| `provider::openrouter` | `openrouter` (depends on `openai`) |
| `provider::together` | `together` (depends on `openai`) |
| `provider::replicate` | `replicate` |
| `provider::openai_compatible` | `openai-compatible` |
| `provider::anthropic_compatible` | `anthropic-compatible` |
| `provider::github_copilot` | `openai-compatible` |

**Provider-specific model enums** (behind matching feature flags):
`OpenAiModel`, `AnthropicModel`, `GoogleModel`, `GrokModel`, `GroqModel`, `MistralModel`, `OllamaModel`, `LmStudioModel`, `OpenAiCompatibleModel`

**OAuth flow implementations**:
- `ClaudeCodeAuth` + `PkceSession`
- `GitHubCopilotAuth`
- `OpenAiCodexAuth`

**Registration functions**:
- `register_default_providers(registry: &mut ProviderRegistry)` — registers a `ProviderFactory` impl for each enabled provider
- `register_default_auth_backends(service: &mut AuthService)` — registers an `AuthBackend` impl for each OAuth provider

### 4. roci Meta-crate

Thin facade re-exporting both crates with default initialization:

```rust
pub use roci_core::*;
pub use roci_providers;

/// Default provider registry with all enabled built-in providers.
pub fn default_registry() -> roci_core::provider::ProviderRegistry {
    let mut registry = roci_core::provider::ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);
    registry
}

/// Default AuthService with all built-in auth backends.
pub fn default_auth_service(
    store: Arc<dyn TokenStore>,
) -> roci_core::auth::AuthService {
    let mut svc = roci_core::auth::AuthService::new(store);
    roci_providers::register_default_auth_backends(&mut svc);
    svc
}
```

### 5. ProviderFactory Trait

```rust
/// Factory for creating ModelProvider instances from a provider key + model ID.
pub trait ProviderFactory: Send + Sync {
    /// Provider key(s) this factory handles (e.g., ["openai", "codex"]).
    fn provider_keys(&self) -> &[&str];

    /// Create a ModelProvider for the given model ID and config.
    fn create(
        &self,
        config: &RociConfig,
        provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError>;

    /// Parse a model ID string into provider-specific representation.
    /// Returns None if unrecognized (registry falls through to Custom).
    fn parse_model(
        &self,
        provider_key: &str,
        model_id: &str,
    ) -> Option<Box<dyn std::any::Any + Send + Sync>>;
}
```

**ProviderRegistry**:

```rust
pub struct ProviderRegistry {
    factories: HashMap<String, Arc<dyn ProviderFactory>>,
}

impl ProviderRegistry {
    pub fn new() -> Self;

    pub fn register(&mut self, factory: Arc<dyn ProviderFactory>) {
        for key in factory.provider_keys() {
            self.factories.insert(key.to_string(), factory.clone());
        }
    }

    pub fn create_provider(
        &self,
        provider_key: &str,
        model_id: &str,
        config: &RociConfig,
    ) -> Result<Box<dyn ModelProvider>, RociError>;

    pub fn has_provider(&self, provider_key: &str) -> bool;
}
```

The existing `create_provider()` free function and the closure-based factory in `agent_loop/runner.rs` are replaced by `ProviderRegistry::create_provider()`.

### 6. AuthBackend Trait

```rust
/// Registerable authentication backend.
pub trait AuthBackend: Send + Sync {
    /// Provider aliases this backend handles (e.g., ["copilot", "github-copilot"]).
    fn aliases(&self) -> &[&str];
    /// Display name (e.g., "GitHub Copilot").
    fn display_name(&self) -> &str;
    /// Token store key (e.g., "github-copilot").
    fn store_key(&self) -> &str;

    /// Start a login flow, returning the appropriate AuthStep.
    fn start_login(
        &self,
        store: &Arc<dyn TokenStore>,
    ) -> impl Future<Output = Result<AuthStep, AuthError>> + Send;

    /// Poll a device-code session (if applicable).
    fn poll_device_code(
        &self,
        store: &Arc<dyn TokenStore>,
        session: &DeviceCodeSession,
    ) -> impl Future<Output = Result<AuthPollResult, AuthError>> + Send;
}
```

`AuthService` becomes a generic orchestrator:

```rust
pub struct AuthService {
    store: Arc<dyn TokenStore>,
    backends: Vec<Arc<dyn AuthBackend>>,
}

impl AuthService {
    pub fn new(store: Arc<dyn TokenStore>) -> Self;
    pub fn register_backend(&mut self, backend: Arc<dyn AuthBackend>);

    // start_login, poll_device_code, get_status, logout, all_statuses
    // all delegate to the matched backend via aliases()
}
```

The hardcoded `ProviderKind` enum and `normalize_provider()` in `service.rs` are removed. Backend lookup by alias replaces them.

### 7. LanguageModel Enum Strategy

The current 12-variant feature-gated enum becomes a simple string-based identifier in `roci-core`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum LanguageModel {
    /// Model resolved by provider key + model ID string.
    /// ProviderRegistry resolves this to a concrete provider.
    Known { provider_key: String, model_id: String },
    /// Unregistered / custom model.
    Custom { provider: String, model_id: String },
}
```

Provider-specific model enums (`OpenAiModel`, `AnthropicModel`, etc.) move to `roci-providers` and are used internally within each `ProviderFactory::create()`. They no longer appear in the core public API.

`ModelSelector::parse()` produces `LanguageModel::Known` for any `"provider:model"` string. Validation of whether the provider is registered happens at `ProviderRegistry::create_provider()` time, not at parse time.

`ModelCapabilities` is returned by `ModelProvider::capabilities()` (already the case), not by `LanguageModel`.

### 8. Feature Flag Strategy

| Feature | Owned by | Effect |
|---------|----------|--------|
| `openai`, `anthropic`, `google`, `grok`, `groq`, `mistral`, `ollama`, `lmstudio`, `azure`, `openrouter`, `together`, `replicate`, `openai-compatible`, `anthropic-compatible` | `roci-providers` | Gates provider transport compilation |
| `all-providers` | `roci-providers` | Enables all provider features |
| `agent`, `audio`, `mcp` | `roci-core` | Gates agent loop, audio, MCP modules |
| `full` | `roci` (meta-crate) | Enables `all-providers` + `agent` + `audio` + `mcp` |

Pass-through from meta-crate:

```toml
[features]
default = ["openai", "anthropic", "google"]
openai = ["roci-providers/openai"]
anthropic = ["roci-providers/anthropic"]
# ...
agent = ["roci-core/agent"]
full = ["roci-providers/all-providers", "roci-core/agent", "roci-core/audio", "roci-core/mcp"]
```

`roci-core` has **no** provider feature flags. It is always provider-agnostic.

### 9. Migration Path

The `roci` meta-crate re-exports `roci_core::*`, so most imports are unchanged:

| Before | After (via meta-crate) | Direct (explicit wiring) |
|--------|------------------------|--------------------------|
| `use roci::prelude::*` | Unchanged | `use roci_core::prelude::*` |
| `use roci::provider::ModelProvider` | Unchanged | `use roci_core::provider::ModelProvider` |
| `use roci::models::LanguageModel` | Unchanged | `use roci_core::models::LanguageModel` |
| `create_provider(model, config)` | `roci::default_registry().create_provider(key, id, config)` | Same, build registry manually |
| `AuthService::new(store)` | `roci::default_auth_service(store)` | `roci_core::auth::AuthService::new(store)` (no built-in backends) |
| `use roci::provider::openai::OpenAiProvider` | `use roci_providers::provider::openai::OpenAiProvider` | Same |
| `use roci::auth::providers::claude_code::*` | `use roci_providers::auth::claude_code::*` | Same |

`roci-cli` and `roci-tools` depend on `roci` (meta-crate). Internal wiring changes are invisible.

No deprecation cycle — clean break, consistent with `cli-soc.md`.

## Consequences

- `roci-core` compiles with zero provider dependencies. Third-party crates can depend on `roci-core` alone.
- `roci-providers` depends on `roci-core` and adds all built-in transports + OAuth flows.
- `roci` remains the recommended dependency for most users.
- `LanguageModel` becomes a simple string-based identifier. Provider-specific model enums become internal to `roci-providers`.
- `AuthService` becomes a generic orchestrator. Hardcoded `ProviderKind`/`normalize_provider()` dispatch is replaced by registered `AuthBackend`s.
- `create_provider()` free function is replaced by `ProviderRegistry::create_provider()`.
- Agent `LoopRunner` takes `Arc<ProviderRegistry>` instead of a factory closure.
- Feature flags pass through from `roci` to `roci-providers`; `roci-core` has no provider features.

## Ownership Boundaries

| Concern | `roci-core` | `roci-providers` | `roci` (meta) |
|---------|-------------|------------------|---------------|
| `ModelProvider` trait | Owns | Implements | Re-exports |
| `ProviderFactory` trait | Owns | Implements | Re-exports |
| `ProviderRegistry` | Owns | Populates | Wires defaults |
| `AuthBackend` trait | Owns | Implements | Re-exports |
| `AuthService` orchestrator | Owns | Populates | Wires defaults |
| Provider transports | — | Owns | Re-exports |
| OAuth flows | — | Owns | Re-exports |
| Provider-specific model enums | — | Owns | Re-exports |
| `LanguageModel` (string-based) | Owns | — | Re-exports |
| `ModelSelector` parsing | Owns | — | Re-exports |
| HTTP utilities | Owns | Uses | Re-exports |
| Generation API, Agent loop | Owns | — | Re-exports + convenience |
| Config, Error, Types | Owns | Uses | Re-exports |
| Feature flags (per-provider) | — | Owns | Pass-through |
| Feature flags (agent/audio/mcp) | Owns | — | Pass-through |

## Related

- Parent epic: `tsq-9wwvzzpg` (Provider registry + roci-providers split)
- Predecessor: `docs/architecture/cli-soc.md` (CLI/core SoC)
- Decisions: `tsq-9wwvzzpg.1`
