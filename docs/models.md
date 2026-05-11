# Models

`roci-core` uses provider-neutral model identifiers:

- `LanguageModel::Known { provider_key, model_id }`
- `LanguageModel::Custom { provider, model_id }`

Model resolution happens in the provider layer; runtime and host app code do not own
provider-specific enum logic directly.

## Model catalog (V1)

`roci_core::models` now includes catalog types:

- `ModelInfo`
- `ModelPolicy`
- `ModelCatalogSource`
- `ModelListOptions`
- `ModelCatalog`

The catalog drives:
- `roci-agent models list`
- provider/model discovery for host apps
- filtering by provider and model-policy

V1 constraints:
- No pricing data in catalog entries.
- No hidden defaults; every entry describes what is known at runtime.

Catalog strategy:
- Built-in providers expose static catalogs from their internal provider model
  capabilities and enums (for known models and capability contracts).
- GitHub Copilot runs opportunistic dynamic discovery when auth is valid.
- Copilot list remains resilient: if discovery is unreachable or unauthorized, it
  falls back to static catalog entries so explicit listings still work.

Provider model enums (`OpenAiModel`, `AnthropicModel`, `GoogleModel`, etc.) live in
`roci-providers` and feed static catalogs.

## Runtime candidates

Agent runtime model selection is expressed as ordered
`Vec<LanguageModel>` candidates:

- `AgentConfig.candidates`
- `RunRequest::with_candidates(...)`
- subagent `profile.models`

Candidate order is stable. `candidates[0]` is tried first, duplicates are
deduped by `(provider, model_id)` with first occurrence winning, and an empty
candidate list fails configuration before provider creation.

`RunRequest::new(model, messages)`, `AgentRuntime::set_model(...)`, and
`AgentRuntime::current_model()` remain single-model migration helpers that map
to the primary candidate.

Retries happen on the active candidate first. Bounded retry may advance to the
next candidate after retry exhaustion for transient failures before any partial
assistant output or tool delta. Persistent retry never advances candidates.

Model health observes real run outcomes only. It does not probe providers,
persist to disk, or reorder candidates.

## CLI usage

`roci-agent` added model list command:

```text
roci-agent models list [--provider PROVIDER] [--json]
```

Examples:

- `roci-agent models list --json`
- `roci-agent models list --provider openai --json`
- `roci-agent models list --provider copilot --json`

Notes:
- No `/model` interactive command exists.
- `--provider` filters listing before host-side dedupe.
- `--json` prints machine-readable entries for `ModelInfo` + policy flags.

## API references

- `crates/roci-core/src/models/mod.rs` (`LanguageModel`, model catalog types)
- `crates/roci-providers/src/models/` (provider-specific static model lists/capabilities)
