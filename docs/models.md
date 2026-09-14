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

Built-in catalogs come from live provider discovery. The returned IDs are
authoritative: new IDs are retained without an SDK release, and an empty response
stays empty. Missing credentials, unsupported discovery, and request failures do
not produce a bundled fallback list. `include_dynamic=false` returns an empty
built-in catalog; `include_static` remains available for custom factories but does
not enable a built-in fallback. Explicit discovery still reports missing
credentials or endpoint configuration when `include_unavailable=true`; that flag
does not turn an unavailable account into a successful empty catalog.

Discovery uses the selected account's credential and associated endpoint. OpenAI,
API-key Grok, Groq, Mistral, OpenRouter, Together, and OpenAI-compatible endpoints
use `/models`. Codex uses its authenticated backend catalog; Anthropic and Gemini
use their native model-list APIs. Local providers query their configured server.
GitHub Copilot and Cursor use their authenticated provider catalogs.

Some native transports do not expose usable discovery:

- Azure generation selects deployment names. Listing those requires Azure
  management credentials that this provider does not configure; discovery returns
  a `ModelDiscoveryUnsupported` error.
- Gemini browser OAuth uses Code Assist, which has no verified model-list API.
  Use an explicit model ID or an API-key account for discovery.
- Native xAI OAuth has no verified catalog endpoint. Use an explicit model ID or
  an API-key account for discovery; its OAuth token is not sent to the API-key
  model-list endpoint.

### Discovery support and launch availability

A configured provider can support explicit model launches without exposing a model
catalog. Factories report `RociError::ModelDiscoveryUnsupported` for this case.
Aggregate discovery skips those providers; an explicit provider listing returns
the explanation. Missing credentials remain subject to `include_unavailable`;
authentication failures, transport errors, and other discovery failures propagate. Hosts do not need provider-specific rules to aggregate catalogs.

Provider model enums (`OpenAiModel`, `AnthropicModel`, `GoogleModel`, etc.) remain
in `roci-providers` as capability defaults and transport rules. They do not decide
which IDs appear in a live catalog. Upstream capability metadata augments those
defaults when supplied; IDs without known metadata still remain in the result.

## Reasoning effort capabilities

`ModelInfo.capabilities.reasoning_effort` is the canonical host contract for
reasoning-effort pickers:

- `supported` is the ordered list of values the exact model accepts.
- `default` is the provider value when a host leaves effort unset.
- `ModelCapabilities::reasoning_effort_options`,
  `supports_reasoning_effort`, and `default_reasoning_effort` let hosts consume
  the contract without provider-specific rules.

An empty `supported` list means Roci cannot expose a portable effort picker for
that model. It does not mean the provider lacks every provider-specific thinking
control. `ReasoningEffortCapabilities::new` rejects a default that is not in the
supported list. Catalog JSON includes the capability data under each model's
`capabilities` object.

The Codex provider discovers models separately from public OpenAI. Its account
catalog supplies reasoning levels, defaults, context limits, input modalities,
and speed tiers. Codex discovery includes a client version because the backend
can gate models by client compatibility. A successful catalog request is the
source of selectable IDs; local presets are not evidence of account access.

The same model ID can have different capabilities on public OpenAI and Codex.
Hosts should use the selected provider's catalog instead of combining capability
profiles by model name. Models added upstream remain discoverable even when they
have no matching SDK enum variant.

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
roci-agent models [--account ACCOUNT] list [--provider PROVIDER] [--include-variants] [--json]
```

Examples:

- `roci-agent models list --json`
- `roci-agent models list --provider openai --json`
- `roci-agent models list --provider copilot --json`
- `roci-agent models --account work list --provider codex --json`
- `roci-agent models list --provider cursor --include-variants --json`

Notes:
- No `/model` interactive command exists.
- `--provider` filters listing before host-side dedupe.
- `--json` prints machine-readable entries for `ModelInfo` + policy flags.
- Cursor lists account-derived model families by default. `--include-variants`
  also shows original upstream variant IDs.

## API references

- `crates/roci-core/src/models/mod.rs` (`LanguageModel`, model catalog types)
- `crates/roci-providers/src/models/` (live discovery adapters and provider capability defaults)
