## Overview
Add provider-neutral model catalog and model switching surface: core model metadata DTOs, registry listing API, static provider catalogs, authenticated/available provider discovery, CLI `models list`, and runtime current/switch helpers.

## Constraints / Non-goals
- Active development: breaking API changes allowed; no compatibility shims.
- Pricing is not core Roci scope now; ignore exact pricing and billing metadata in V1.
- Static catalogs may be incomplete; dynamic listing only where provider supports it.
- CLI lists only available providers: registered + compiled local/no-credential providers, and credentialed remote providers with auth/config available.
- Unauthenticated explicit `--provider <remote>` returns typed auth/config error; unauthenticated remotes are hidden from all-provider list.
- Runtime model switching remains idle-only; no validation/network call in `switch_model`.

## Interfaces (CLI/API)
- `ModelInfo`, `ModelPolicy`, `ModelCatalogSource`, `ModelListOptions`, `ModelCatalog` in `roci_core::models::catalog`.
- `ProviderFactory::list_models(config, provider_key, options)` async API.
- `ProviderRegistry::list_models(config, options)`.
- `AgentRuntime::current_model()` and `AgentRuntime::switch_model(model) -> previous_model`; `set_model` delegates.
- CLI: `roci-agent models list --provider <provider> --json`.

## Data model / schema changes
- `ModelInfo` includes provider key, model id, display name, capabilities, policy, source, and metadata.
- Capability shape waits on attachment media/file limit contract so catalogs do not freeze stale schema.
- `ModelCatalog` dedupes by `(provider_key, model_id)` with deterministic first/priority wins.
- Provider static catalogs derive capabilities from existing provider model enums where possible.
- Copilot parser accepts OpenAI-style `{ data: [...] }` and raw array model lists.

## Acceptance criteria
- Core catalog serde/dedupe tests pass.
- Provider static catalog snapshot tests pass.
- Copilot mock `/models` tests pass.
- CLI JSON test proves unauthenticated remotes are hidden from all-provider listing and explicit unauth remote returns typed auth/config error.
- Runtime current/switch model tests pass.
- Docs and Copilot model-list live tmux smoke complete when provider auth available.

## Test plan
- `cargo test -p roci-core catalog`
- `cargo test -p roci-core --features agent "agent::runtime::tests::state_lifecycle"`
- `cargo test -p roci-providers --features all-providers catalog`
- `cargo test -p roci-providers --features github-copilot github_copilot_models`
- `cargo test -p roci-cli parse_models`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --features full -- -D warnings`
- `cargo test`
- Live tmux: `roci-agent models list --provider github-copilot --json` only if authenticated.
