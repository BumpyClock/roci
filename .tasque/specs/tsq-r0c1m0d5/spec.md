# Provider-Neutral Model Catalog and Switching Surface Implementation Plan

> **For agentic workers:** Execute task-by-task. Use subagent-driven development when available, otherwise run tasks inline with review checkpoints. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add provider-neutral model catalog APIs, built-in/static + Copilot dynamic model listing, CLI `models list`, and runtime model switching polish.

**Architecture:** `roci-core` owns catalog DTOs, dedupe/filtering, and registry aggregation. `roci-providers` owns concrete static catalogs and Copilot `/models` transport. `roci-cli` consumes registry APIs only; runtime switching stays idle-only and provider-agnostic.

**Tech Stack:** Rust 2021, `serde`, `async-trait`, `reqwest`, `wiremock`, `clap`, `tokio`, `tmux` live verification.

---

## Current Architecture Facts

- `crates/roci-core/src/models/` currently exposes `LanguageModel`, `ModelSelector`, `ProviderKey`, and `ModelCapabilities`; no catalog module exists.
- `ProviderFactory` is sync today: `provider_keys`, `requires_credentials`, `create`; `ProviderRegistry` stores `HashMap<String, Arc<dyn ProviderFactory>>` and has no listing API.
- Built-in provider model enums in `crates/roci-providers/src/models/*.rs` already encode `as_str()` and `capabilities()`, but no iterable/static list exists.
- `GitHubCopilotProvider` wraps `OpenAiCompatibleProvider` and centralizes Copilot headers in `provider/github_copilot.rs`; factory credential resolution lives in `factories.rs` using token-store key `github-copilot-api`.
- `OverflowClassifyingFactory` wraps all built-in factories and must delegate new list APIs plus existing `requires_credentials`.
- CLI commands are `auth`, `audio`, `chat`, `skills`; dispatch lives in `crates/roci-cli/src/main.rs`.
- `AgentRuntime::set_model` exists in `runtime/mutations.rs`, idle-only, no provider validation; no public `current_model` or `switch_model` exists.

## Public API / Types

Add `crates/roci-core/src/models/catalog.rs`, export from `models/mod.rs` and prelude if useful:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ModelInfo {
    pub provider_key: String,
    pub model_id: String,
    pub display_name: Option<String>,
    pub capabilities: ModelCapabilities,
    pub policy: ModelPolicy,
    pub billing: ModelBilling,
    pub source: ModelCatalogSource,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ModelPolicy {
    pub is_default: bool,
    pub is_deprecated: bool,
    pub is_preview: bool,
    pub requires_credentials: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelBilling {
    Unknown,
    Included,
    Metered,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelCatalogSource {
    Static,
    Dynamic,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ModelListOptions {
    pub provider: Option<String>,
    pub include_static: bool,
    pub include_dynamic: bool,
    pub include_deprecated: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ModelCatalog {
    pub models: Vec<ModelInfo>,
}
```

Required impls: `Default` for `ModelPolicy` (`false/Unknown` semantics except `requires_credentials=true` via factory), `Default` for `ModelListOptions` (`provider=None`, `include_static=true`, `include_dynamic=false`, `include_deprecated=false`), `ModelInfo::selector() -> LanguageModel`, `ModelInfo::qualified_id() -> String`, and `ModelCatalog::dedupe_first_wins()` keyed by `(provider_key, model_id)`.

Extend `ProviderFactory` with async default listing:

```rust
#[async_trait::async_trait]
pub trait ProviderFactory: Send + Sync {
    fn provider_keys(&self) -> &[&str];
    fn requires_credentials(&self, provider_key: &str) -> bool { true }
    fn create(&self, config: &RociConfig, provider_key: &str, model_id: &str) -> Result<Box<dyn ModelProvider>, RociError>;
    async fn list_models(&self, config: &RociConfig, provider_key: &str, options: &ModelListOptions) -> Result<Vec<ModelInfo>, RociError> { Ok(Vec::new()) }
}
```

Add `ProviderRegistry::list_models(&self, config: &RociConfig, options: ModelListOptions) -> Result<Vec<ModelInfo>, RociError>`. Filter exact registered provider key when `options.provider` is set; otherwise aggregate all registered keys. Sort output by `(provider_key, model_id)` after first-wins dedupe.

Runtime API: add `AgentRuntime::current_model(&self) -> LanguageModel` and `AgentRuntime::switch_model(&self, model: LanguageModel) -> Result<LanguageModel, RociError>` returning previous model. Keep `set_model` as delegate to `switch_model` and do not provider-validate in mutator.

## File / Module Changes

- Create `crates/roci-core/src/models/catalog.rs`: DTOs, defaults, serde/dedupe tests.
- Modify `crates/roci-core/src/models/mod.rs`: `pub mod catalog; pub use catalog::*;`.
- Modify `crates/roci-core/src/provider/factory.rs`: add async `list_models` default.
- Modify `crates/roci-core/src/provider/registry.rs`: add async aggregate/filter/dedupe tests.
- Modify `crates/roci-providers/src/models/catalog.rs`: create static catalog builders for enabled enum-backed providers.
- Modify `crates/roci-providers/src/models/mod.rs`: export `catalog`.
- Modify `crates/roci-providers/src/factories.rs`: each built-in factory returns static catalog; `GitHubCopilotFactory` includes dynamic when requested.
- Modify `crates/roci-providers/src/provider/github_copilot.rs`: factor `copilot_headers()`, add `/models` client and response parser.
- Modify `crates/roci-providers/src/overflow.rs`: delegate `requires_credentials` and `list_models` to wrapped factory.
- Modify `crates/roci-cli/src/cli/mod.rs`: add `Models(ModelsArgs)` / `ModelsCommands::List(ModelListArgs)` with `--provider` and `--json`.
- Create `crates/roci-cli/src/models_cmd.rs`, modify `crates/roci-cli/src/main.rs`: command handler and JSON/human formatting.
- Modify `crates/roci-core/src/agent/runtime/mutations.rs` and `state.rs` if needed: `switch_model`, `current_model`.
- Update `docs/models.md` and `docs/testing.md` with catalog API, CLI examples, Copilot live smoke.

## Dependency Order / Child Tasks

1. `tsq-r0c1m0d5.1` Define roci-core model catalog API and registry listing contract. Blocks all implementation slices.
2. `tsq-r0c1m0d5.2` Add static built-in provider model catalogs. Blocks docs/final verification.
3. `tsq-r0c1m0d5.3` Implement dynamic GitHub Copilot `/models` discovery. Blocks docs/final verification.
4. `tsq-r0c1m0d5.4` Add `roci-agent models list` command. Starts after core; should integrate provider static/dynamic before final review.
5. `tsq-r0c1m0d5.5` Polish `AgentRuntime` `current_model` / `switch_model` surface. Starts after core; parallel with provider work.
6. `tsq-r0c1m0d5.6` Update docs and run verification gates. Starts after provider, CLI, runtime slices.

Parallel after Task 1: Tasks 2, 3, 5. Task 4 can start after Task 1 with a mock registry, then integrate after Tasks 2-3.

## Tests

- Core: `cargo test -p roci-core catalog`; verify serde roundtrip, default options, first-wins dedupe, `qualified_id`, registry provider filter, empty catalog for unknown dynamic default, and stable sort.
- Runtime: `cargo test -p roci-core --features agent "agent::runtime::tests::state_lifecycle"`; add `current_model_returns_clone`, `switch_model_returns_previous_when_idle`, `switch_model_rejects_when_running`, and keep existing `set_model` test green.
- Providers static: `cargo test -p roci-providers --features all-providers catalog`; assert inline JSON/id snapshots for OpenAI, Anthropic, Google, Grok, Groq, Mistral, Ollama; assert custom-only providers return empty static catalog.
- Copilot dynamic: `cargo test -p roci-providers --features github-copilot github_copilot_models`; use `wiremock` GET `/models`, token-store `github-copilot-api` with unexpired token and mock base URL in `account_id`; assert Copilot headers, parsed IDs, source `dynamic`, dedupe against static.
- CLI: `cargo test -p roci-cli parse_models`; parse `roci-agent models list`, `--provider github-copilot`, `--json`. Add formatter test asserting JSON is valid `Vec<ModelInfo>` and human output contains provider/model IDs.

## Docs / Live Verification

- Docs: `docs/models.md` gets API and CLI usage; `docs/testing.md` gets Copilot model-list smoke.
- Full gates before handoff: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --features full -- -D warnings`; `cargo test`; targeted commands above.
- Live tmux smoke after provider-facing code lands:

```bash
tmux new-session -d -s roci-copilot-models \
  'cd /Users/adityasharma/Projects/roci && \
   cargo run -q -p roci-cli -- models list --provider github-copilot --json; \
   status=$?; printf "\n[roci copilot models exit=%s]\n" "$status"; exec zsh'
echo "attach: tmux attach -t roci-copilot-models"
```

Expected: JSON array with at least one `provider_key == "github-copilot"`, exit `0`. If auth missing, run `roci-agent auth login copilot` interactively first, then rerun smoke.

## Risks / Open Questions

- Copilot `/models` is provider-facing and may change shape; parser should accept OpenAI-style `{ "data": [{ "id": ... }] }` plus raw array shape, and live smoke is required.
- Static catalogs can stale; keep pricing out of v1 by using `ModelBilling::{Unknown, Included, Metered}` only.
- Default `ModelListOptions` must not do network; CLI opts into Copilot dynamic listing when `--provider github-copilot` is present.
- Async method on `ProviderFactory` is breaking but acceptable in active development; update all test factories only if compiler requires annotation.