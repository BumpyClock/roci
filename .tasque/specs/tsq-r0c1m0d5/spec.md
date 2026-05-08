# Model Catalog And Switching Design

## Task

`tsq-r0c1m0d5` adds provider-neutral model catalog and switching foundations:
core model metadata DTOs, provider/registry listing APIs, static built-in catalogs,
opportunistic Copilot dynamic discovery, `roci-agent models list`, and runtime
current/switch helpers.

## Scope

V1 is a foundation layer, not an interactive chat command.

Included:

- `roci-core` catalog types and registry aggregation.
- `roci-providers` static model catalogs from existing model enums/capabilities.
- GitHub Copilot model discovery when authenticated/configured, with static fallback.
- `roci-agent models list [--provider PROVIDER] [--json]` as a real CLI harness for the catalog API.
- `AgentRuntime::current_model()` and idle-only `AgentRuntime::switch_model(model)`.

Not included:

- interactive `/model` inside `roci-agent chat`,
- model picker UI,
- config persistence for selected models,
- model catalog cache files,
- provider pricing/billing metadata.

## Context

Roci currently parses model selectors as `provider:model` strings and resolves
providers through `ProviderRegistry::create_provider`. Provider-specific enums
already know model capabilities, but there is no provider-neutral list API for
apps or CLI tools to inspect available models.

Pi and Codex both keep catalog, availability, and active model state separate:

- Pi uses a static/generated catalog plus runtime overlays and auth-aware available lists.
- Codex uses a bundled seed catalog plus optional remote refresh/cache for some providers.
- Both keep runtime switching as session state, not provider construction.

Roci should take smaller V1: static catalogs plus provider list APIs, no cache
manager yet. Dynamic discovery is provider-local and opportunistic.

## Design Decision

Use three layers:

```text
static/provider catalog
  -> registry aggregation/filtering
  -> runtime current/switch state
```

This keeps `models list`, future UI pickers, and runtime switching independent.
Listing models should not mutate runtime state. Switching models should not call
provider APIs or validate credentials.

## Core Catalog API

Add `roci_core::models::catalog` with:

- `ModelInfo`
- `ModelPolicy`
- `ModelCatalogSource`
- `ModelListOptions`
- `ModelCatalog`

Recommended V1 shape:

```text
ModelInfo {
    provider_key: String,
    model_id: String,
    display_name: Option<String>,
    capabilities: ModelCapabilities,
    policy: ModelPolicy,
    source: ModelCatalogSource,
    metadata: BTreeMap<String, serde_json::Value>,
}
```

`ModelPolicy` should capture app-relevant constraints without pricing:

- `requires_credentials: bool`
- `local: bool`
- `deprecated: bool`
- `default_for_provider: bool`

`ModelCatalogSource` should identify where model metadata came from:

- `Static`
- `Dynamic { endpoint: String }`

`ModelListOptions` should include:

- optional `provider_key`,
- `include_dynamic`,
- `include_static`,
- `include_unavailable`.

`ModelCatalog` should dedupe by `(provider_key, model_id)` with deterministic
ordering. Dynamic entries win over static entries for the same pair because they
represent provider-reported availability.

## Provider And Registry Contracts

Extend `ProviderFactory` with an async listing API:

```text
async fn list_models(
    &self,
    config: &RociConfig,
    provider_key: &str,
    options: &ModelListOptions,
) -> Result<ModelCatalog, RociError>
```

Default implementation may return an empty catalog so custom providers do not
need immediate work. Keep the trait object-safe via the repo's existing async
trait pattern or an explicit boxed future.

`ProviderRegistry::list_models(config, options)` should:

- list one provider when `provider_key` is set,
- aggregate all registered provider keys when unset,
- hide unauthenticated remote providers from all-provider listings,
- return typed auth/config errors for explicit unauthenticated remote provider requests,
- keep no-credential local providers visible.

Provider registry should not persist model lists or cache network results in V1.

## Built-In Static Catalogs

Add `roci-providers` static catalog helpers that map existing provider model enums
to `ModelInfo`:

- OpenAI/Codex from `OpenAiModel`,
- Anthropic from `AnthropicModel`,
- Google from `GoogleModel`,
- Grok/Groq/Mistral/Ollama/LM Studio where enums already exist,
- compatible/router providers as either empty catalogs or conservative static
  examples only when current code already has a known model enum.

Capabilities must come from each model enum's existing `capabilities()` method
where available. This avoids stale duplicate capability tables.

## GitHub Copilot Discovery

GitHub Copilot gets static fallback plus opportunistic dynamic discovery.

Behavior:

- All-provider list without Copilot auth hides Copilot remote dynamic results.
- Explicit `--provider github-copilot` tries dynamic discovery when auth/config exists.
- Dynamic parser accepts OpenAI-style `{ "data": [...] }` and raw array responses.
- If dynamic discovery is unavailable but static Copilot fallback exists, explicit
  Copilot listing returns static entries with `source=Static`.
- Unauthenticated all-provider listing does not include Copilot static fallback;
  hiding unauthenticated remote providers takes priority for broad discovery.
- If neither dynamic credentials nor static fallback can produce entries, return
  typed auth/config error for explicit provider listing.

Both Pi and Codex scouts found no proven Copilot `/models` precedent in their
codebases, so dynamic discovery must be tested against mocks and live smoke only
when authenticated provider access is available.

## Runtime Switching API

Add runtime helpers:

- `AgentRuntime::current_model() -> LanguageModel`
- `AgentRuntime::switch_model(model: LanguageModel) -> Result<LanguageModel, RociError>`
- `AgentRuntime::set_model(model)` delegates to `switch_model(model)` and discards previous model.

Switch semantics:

- allowed only when runtime is idle,
- returns previous model,
- does not validate provider registration,
- does not perform auth/network calls,
- affects subsequent turns only.

This keeps runtime switching cheap and predictable for future host UIs.

## CLI Surface

Add top-level command:

```bash
roci-agent models list [--provider PROVIDER] [--json]
```

Human output should be deterministic and compact:

```text
PROVIDER        MODEL                         CONTEXT   TOOLS   VISION   SOURCE
openai          gpt-4o                        128000    yes     yes      static
```

JSON output should serialize `ModelCatalog` or an equivalent stable wrapper.

`roci-cli` must call the real `ProviderRegistry::list_models` API. Parser-only
tests are insufficient because CLI exists here to prove actual SDK behavior.

## Error Handling

Use existing `RociError` variants where they fit:

- missing credentials -> `MissingCredential` or provider auth error,
- missing provider config/base URL -> `MissingConfiguration`,
- unregistered provider -> `ModelNotFound` or existing registry error shape.

All-provider listing should skip unavailable remote providers rather than fail
the whole command. Explicit provider listing should fail loudly when user asked
for a specific unavailable remote and no static fallback is available.

## Test Plan

Core:

- catalog serde round trip,
- catalog deterministic ordering and dedupe,
- registry provider filtering,
- all-provider listing hides unavailable remotes,
- explicit unavailable remote returns typed error,
- local/no-credential providers remain visible,
- runtime `current_model`,
- runtime `switch_model` returns previous,
- `switch_model` rejects while running,
- `set_model` delegates.

Providers:

- static catalog snapshot tests by provider,
- capabilities in catalog match enum capabilities,
- Copilot parser handles `{ data: [...] }`,
- Copilot parser handles raw arrays,
- Copilot dynamic discovery mock test,
- Copilot static fallback test.

CLI:

- parse `models list`,
- parse `models list --provider openai --json`,
- command JSON calls real registry API,
- all-provider output hides unavailable remote providers,
- explicit unavailable remote provider returns typed help/error,
- static/local provider list succeeds without credentials.

Verification gates:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features full -- -D warnings
cargo test -p roci-core catalog
cargo test -p roci-core --features agent state_lifecycle
cargo test -p roci-providers catalog
cargo test -p roci-providers --features github-copilot github_copilot_models
cargo test -p roci-cli models
cargo test --workspace --all-targets
```

Live verification:

- run `roci-agent models list --provider lmstudio --json` against local LM Studio when available,
- run `roci-agent models list --provider github-copilot --json` in tmux when authenticated,
- if Copilot auth is unavailable, record that live dynamic smoke was unavailable
  and prove static/local catalog via `roci-agent models list --json`.

## Parallel Implementation Shape

Order:

1. `.1` core API and registry listing contract first.
2. `.2`, `.3`, `.4`, and `.5` can run mostly in parallel after `.1`.
3. `.6` docs and verification last.

Suggested ownership:

- `.1`: `crates/roci-core/src/models/catalog.rs`,
  `crates/roci-core/src/provider/{factory,registry}.rs`,
  `crates/roci-core/src/models/mod.rs`.
- `.2`: `crates/roci-providers/src/models/*`,
  `crates/roci-providers/src/models/catalog.rs`,
  `crates/roci-providers/src/factories.rs`.
- `.3`: `crates/roci-providers/src/provider/github_copilot.rs`,
  Copilot factory/listing tests.
- `.4`: `crates/roci-cli/src/cli/mod.rs`,
  new `models_cmd` module, `crates/roci-cli/src/main.rs`.
- `.5`: `crates/roci-core/src/agent/runtime/mutations.rs`,
  runtime state lifecycle tests.
- `.6`: `docs/models.md`, `docs/ARCHITECTURE.md`, `docs/testing.md`,
  final test/live evidence.

## Follow-Up

Track later tasks for:

- interactive `/model` command or picker in a host app,
- model catalog cache/refresh manager,
- user config persistence for selected model,
- generated model catalog pipeline,
- pricing/billing metadata,
- provider-specific reasoning/service-tier picker behavior.
