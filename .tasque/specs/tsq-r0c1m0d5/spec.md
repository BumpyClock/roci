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

`ModelListOptions::default()` should be:

```text
provider_key: None
include_dynamic: true
include_static: true
include_unavailable: false
```

`include_unavailable=false` hides remote providers requiring credentials when
credentials/config are absent. `include_unavailable=true` may include static
remote entries marked `requires_credentials=true`, but must not call provider
endpoints without credentials.

`ModelCatalog` should dedupe by `(provider_key, model_id)` with deterministic
ordering. Dynamic entries win over static entries for the same pair because they
represent provider-reported availability.

## Provider And Registry Contracts

Extend `ProviderFactory` with an object-safe listing API:

```text
fn list_models<'a>(
    &self,
    config: &'a RociConfig,
    provider_key: &'a str,
    options: &'a ModelListOptions,
) -> BoxFuture<'a, Result<ModelCatalog, RociError>>
```

Default implementation may return an empty catalog so custom providers do not
need immediate work. Do not use native `async fn` in this dyn trait because
`ProviderFactory` is stored as `Arc<dyn ProviderFactory>`.

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
- Unauthenticated all-provider listing does not include Copilot static fallback;
  hiding unauthenticated remote providers takes priority for broad discovery.
- Missing credentials/config: explicit provider may return static fallback;
  all-provider hides Copilot.
- 404/405 from `/models`: explicit provider returns static fallback.
- 401/403 with credentials: return typed auth error, no fallback.
- 2xx parse error: return typed provider/config error, no fallback.
- network timeout/5xx: return static fallback only when `include_static=true`,
  and attach warning metadata to the returned static entries.
- If neither dynamic credentials nor static fallback can produce entries, return
  typed auth/config error for explicit provider listing.

Both Pi and Codex scouts found no proven Copilot `/models` precedent in their
codebases, so dynamic discovery must be tested against mocks and live smoke only
when authenticated provider access is available.

## Runtime Switching API

Add runtime helpers:

- `pub async fn current_model(&self) -> LanguageModel`
- `pub async fn switch_model(&self, model: LanguageModel) -> Result<LanguageModel, RociError>`
- `pub async fn set_model(&self, model: LanguageModel) -> Result<(), RociError>`
  delegates to `switch_model(model)` and discards previous model.

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

JSON output should use a stable wrapper:

```json
{
  "models": []
}
```

`roci-cli` must call the real `ProviderRegistry::list_models` API. Parser-only
tests are insufficient because CLI exists here to prove actual SDK behavior.
Add an injectable runner shape so tests can prove this:

```text
models_cmd::run(args, registry: Arc<ProviderRegistry>, config: RociConfig, writer)
```

CLI tests should pass a stub `ProviderFactory::list_models` that records
invocation and returns sentinel catalog data. Assertions must check that JSON
contains the sentinel model from registry output, not hard-coded fixture text.

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

Live verification must run in tmux and print the attach command before the
provider-facing run.

Live checks:

- run `cargo run -q -p roci-cli -- models list --provider openai --json`
  to prove static registry listing,
- run `cargo run -q -p roci-cli --features roci/github-copilot -- models list --provider github-copilot --json`
  in tmux when authenticated,
- if Copilot auth is unavailable, record that live dynamic smoke was unavailable
  and prove static/local catalog via `roci-agent models list --json`.

## Parallel Implementation Shape

Order:

1. `.1` core API and registry listing contract first.
2. `.2`, `.3`, and `.5` can run mostly in parallel after `.1`.
3. `.4a` CLI parser and injectable handler can start after `.1` using a stub registry.
4. `.4b` built-in provider CLI behavior follows `.2`; Copilot CLI behavior follows `.3`.
5. `.6` docs and verification last.

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
