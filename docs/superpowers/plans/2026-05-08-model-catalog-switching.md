# Model Catalog And Switching Implementation Plan

> **For agentic workers:** Prefer `subagent-driven-development` for execution when available. Task implementers own task work and review fixes; integration owner owns final integration. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build V1 provider-neutral model catalog, `roci-agent models list`, and idle-only runtime model switching.

**Architecture:** Add core catalog DTOs and object-safe provider listing contract first. Built-in providers then map existing model enums/capabilities into static catalogs, Copilot adds opportunistic `/models` discovery with static fallback, CLI calls the real registry API, and runtime switching remains a cheap state mutation independent of provider validation.

**Tech Stack:** Rust 2021, `roci-core`, `roci-providers`, `roci-cli`, `futures::future::BoxFuture`, `serde`, `wiremock`, `tokio`, `clap`.

---

## File Structure

- Create `crates/roci-core/src/models/catalog.rs`: provider-neutral catalog DTOs, options, dedupe/order helpers, serde tests.
- Modify `crates/roci-core/src/models/mod.rs`: export `catalog`.
- Modify `crates/roci-core/src/provider/factory.rs`: add object-safe `list_models` default method returning `BoxFuture`.
- Modify `crates/roci-core/src/provider/registry.rs`: aggregate/filter provider catalogs and add tests.
- Modify `crates/roci-providers/src/overflow.rs`: delegate `list_models` through `OverflowClassifyingFactory`.
- Create `crates/roci-providers/src/models/catalog.rs`: static catalog helpers and per-provider catalog builders.
- Modify `crates/roci-providers/src/models/mod.rs`: export provider catalog module.
- Modify `crates/roci-providers/src/factories.rs`: each built-in factory delegates `list_models` to static/dynamic catalog helpers.
- Modify `crates/roci-providers/src/provider/github_copilot.rs`: add Copilot `/models` client/parser helpers and tests.
- Modify `crates/roci-core/src/agent/runtime/mutations.rs`: add `current_model` and `switch_model`; keep `set_model`.
- Modify `crates/roci-core/src/agent/runtime_tests/state_lifecycle.rs`: runtime switching tests.
- Modify `crates/roci-cli/src/cli/mod.rs`: add `models list` args and parse tests.
- Create `crates/roci-cli/src/models_cmd.rs`: injectable command runner that calls registry API and renders table/JSON.
- Modify `crates/roci-cli/src/main.rs`: wire `Commands::Models`.
- Modify docs: `docs/models.md`, `docs/ARCHITECTURE.md`, `docs/testing.md`.

---

### Task 1: Core Catalog API And Registry Listing Contract (`tsq-r0c1m0d5.1`)

**Files:**
- Create: `crates/roci-core/src/models/catalog.rs`
- Modify: `crates/roci-core/src/models/mod.rs`
- Modify: `crates/roci-core/src/provider/factory.rs`
- Modify: `crates/roci-core/src/provider/registry.rs`

- [ ] **Step 1: Write catalog DTO tests**

Add tests in `crates/roci-core/src/models/catalog.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ModelCapabilities, ModelInputCapabilities};

    fn model(provider: &str, id: &str, source: ModelCatalogSource) -> ModelInfo {
        ModelInfo {
            provider_key: provider.to_string(),
            model_id: id.to_string(),
            display_name: Some(id.to_string()),
            capabilities: ModelCapabilities {
                supports_streaming: true,
                supports_system_messages: true,
                context_length: 128_000,
                input: ModelInputCapabilities::default(),
                ..ModelCapabilities::default()
            },
            policy: ModelPolicy {
                requires_credentials: true,
                local: false,
                deprecated: false,
                default_for_provider: false,
            },
            source,
            metadata: Default::default(),
        }
    }

    #[test]
    fn model_list_options_default_hides_unavailable_and_includes_sources() {
        let options = ModelListOptions::default();
        assert!(options.provider_key.is_none());
        assert!(options.include_dynamic);
        assert!(options.include_static);
        assert!(!options.include_unavailable);
    }

    #[test]
    fn model_catalog_dedupes_dynamic_over_static() {
        let mut catalog = ModelCatalog::default();
        catalog.insert(model("openai", "gpt-4o", ModelCatalogSource::Static));
        catalog.insert(model(
            "openai",
            "gpt-4o",
            ModelCatalogSource::Dynamic {
                endpoint: "/models".to_string(),
            },
        ));

        let models = catalog.into_models();
        assert_eq!(models.len(), 1);
        assert!(matches!(models[0].source, ModelCatalogSource::Dynamic { .. }));
    }

    #[test]
    fn model_catalog_round_trips_json() {
        let mut catalog = ModelCatalog::default();
        catalog.insert(model("openai", "gpt-4o", ModelCatalogSource::Static));

        let json = serde_json::to_string(&catalog).unwrap();
        let decoded: ModelCatalog = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.models().len(), 1);
        assert_eq!(decoded.models()[0].provider_key, "openai");
    }
}
```

- [ ] **Step 2: Run failing catalog test**

Run:

```bash
cargo test -p roci-core catalog
```

Expected before implementation: compile failure for missing `catalog` types.

- [ ] **Step 3: Implement catalog DTOs**

Create `crates/roci-core/src/models/catalog.rs` with:

```rust
//! Provider-neutral model catalog types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::ModelCapabilities;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelInfo {
    pub provider_key: String,
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub capabilities: ModelCapabilities,
    pub policy: ModelPolicy,
    pub source: ModelCatalogSource,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelPolicy {
    pub requires_credentials: bool,
    pub local: bool,
    pub deprecated: bool,
    pub default_for_provider: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelCatalogSource {
    Static,
    Dynamic { endpoint: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelListOptions {
    pub provider_key: Option<String>,
    pub include_dynamic: bool,
    pub include_static: bool,
    pub include_unavailable: bool,
}

impl Default for ModelListOptions {
    fn default() -> Self {
        Self {
            provider_key: None,
            include_dynamic: true,
            include_static: true,
            include_unavailable: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelCatalog {
    models: Vec<ModelInfo>,
}

impl ModelCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_models(models: impl IntoIterator<Item = ModelInfo>) -> Self {
        let mut catalog = Self::new();
        for model in models {
            catalog.insert(model);
        }
        catalog
    }

    pub fn models(&self) -> &[ModelInfo] {
        &self.models
    }

    pub fn update_models(&mut self, mut update: impl FnMut(&mut ModelInfo)) {
        for model in &mut self.models {
            update(model);
        }
        self.normalize();
    }

    pub fn into_models(self) -> Vec<ModelInfo> {
        self.models
    }

    pub fn insert(&mut self, model: ModelInfo) {
        if let Some(existing) = self.models.iter_mut().find(|existing| {
            existing.provider_key == model.provider_key && existing.model_id == model.model_id
        }) {
            if source_rank(&model.source) >= source_rank(&existing.source) {
                *existing = model;
            }
        } else {
            self.models.push(model);
        }
        self.models.sort_by(|a, b| {
            a.provider_key
                .cmp(&b.provider_key)
                .then_with(|| a.model_id.cmp(&b.model_id))
        });
    }

    pub fn extend(&mut self, other: ModelCatalog) {
        for model in other.into_models() {
            self.insert(model);
        }
    }
}

fn source_rank(source: &ModelCatalogSource) -> u8 {
    match source {
        ModelCatalogSource::Static => 0,
        ModelCatalogSource::Dynamic { .. } => 1,
    }
}
```

Export from `crates/roci-core/src/models/mod.rs`:

```rust
pub mod catalog;
pub use catalog::{ModelCatalog, ModelCatalogSource, ModelInfo, ModelListOptions, ModelPolicy};
```

- [ ] **Step 4: Add object-safe `ProviderFactory::list_models`**

Modify `crates/roci-core/src/provider/factory.rs`:

```rust
use futures::future::{ready, BoxFuture};

use crate::models::{ModelCatalog, ModelListOptions};

pub trait ProviderFactory: Send + Sync {
    fn provider_keys(&self) -> &[&str];

    fn requires_credentials(&self, _provider_key: &str) -> bool {
        true
    }

    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        _provider_key: &'a str,
        _options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(ready(Ok(ModelCatalog::default())))
    }

    fn create(
        &self,
        config: &RociConfig,
        provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError>;
}
```

- [ ] **Step 5: Add registry list tests**

In `crates/roci-core/src/provider/registry.rs` tests, add stub factories:

```rust
#[test]
fn provider_keys_are_sorted_for_deterministic_listing() {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(StubFactory));

    assert_eq!(registry.provider_keys(), vec!["stub", "stub-alias"]);
}

#[tokio::test]
async fn list_models_filters_by_provider_key() {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(StubFactory));
    let config = RociConfig::new().with_token_store(None);
    let options = ModelListOptions {
        provider_key: Some("stub".to_string()),
        ..ModelListOptions::default()
    };

    let catalog = registry.list_models(&config, &options).await.unwrap();

    assert_eq!(catalog.models().len(), 1);
    assert_eq!(catalog.models()[0].provider_key, "stub");
}

#[tokio::test]
async fn list_models_hides_unavailable_remote_in_all_provider_mode() {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(CredentialedCatalogFactory));
    let config = RociConfig::new().with_token_store(None);

    let catalog = registry
        .list_models(&config, &ModelListOptions::default())
        .await
        .unwrap();

    assert!(catalog.models().is_empty());
}

#[tokio::test]
async fn explicit_unavailable_remote_returns_auth_error() {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(CredentialedCatalogFactory));
    let config = RociConfig::new().with_token_store(None);
    let options = ModelListOptions {
        provider_key: Some("remote".to_string()),
        ..ModelListOptions::default()
    };

    let err = registry.list_models(&config, &options).await.unwrap_err();

    assert!(matches!(err, RociError::MissingCredential { provider } if provider == "remote"));
}

#[tokio::test]
async fn credentialed_remote_appears_in_all_provider_mode() {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(CredentialedCatalogFactory));
    let config = RociConfig::new().with_token_store(None);
    config.set_api_key("remote", "token".to_string());

    let catalog = registry
        .list_models(&config, &ModelListOptions::default())
        .await
        .unwrap();

    assert_eq!(catalog.models().len(), 1);
    assert_eq!(catalog.models()[0].provider_key, "remote");
}
```

Use a helper model:

```rust
fn catalog_model(provider_key: &str, model_id: &str, requires_credentials: bool) -> ModelInfo {
    ModelInfo {
        provider_key: provider_key.to_string(),
        model_id: model_id.to_string(),
        display_name: None,
        capabilities: ModelCapabilities::default(),
        policy: ModelPolicy {
            requires_credentials,
            local: !requires_credentials,
            deprecated: false,
            default_for_provider: false,
        },
        source: ModelCatalogSource::Static,
        metadata: Default::default(),
    }
}

struct CredentialedCatalogFactory;

impl ProviderFactory for CredentialedCatalogFactory {
    fn provider_keys(&self) -> &[&str] {
        &["remote"]
    }

    fn list_models<'a>(
        &'a self,
        config: &'a RociConfig,
        provider_key: &'a str,
        _options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(async move {
            if config.get_api_key(provider_key).is_none() {
                return Err(RociError::MissingCredential {
                    provider: provider_key.to_string(),
                });
            }
            Ok(ModelCatalog::from_models([catalog_model(
                provider_key,
                "remote-model",
                true,
            )]))
        })
    }

    fn create(
        &self,
        _config: &RociConfig,
        _provider_key: &str,
        _model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        unreachable!("catalog tests must not create providers")
    }
}

impl ProviderFactory for StubFactory {
    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        provider_key: &'a str,
        _options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(async move {
            Ok(ModelCatalog::from_models([catalog_model(
                provider_key,
                "stub-model",
                false,
            )]))
        })
    }
}
```

- [ ] **Step 6: Implement `ProviderRegistry::list_models`**

Add to `crates/roci-core/src/provider/registry.rs`:

```rust
fn has_credentials(config: &RociConfig, provider_key: &str) -> bool {
    config.get_api_key(provider_key).is_some()
}

pub async fn list_models(
    &self,
    config: &RociConfig,
    options: &ModelListOptions,
) -> Result<ModelCatalog, RociError> {
    if let Some(provider_key) = options.provider_key.as_deref() {
        let factory = self.factories.get(provider_key).ok_or_else(|| {
            RociError::ModelNotFound(format!(
                "No provider factory registered for '{provider_key}'"
            ))
        })?;
        // Explicit provider listing delegates availability/fallback policy to the
        // factory. This allows providers with static fallback, such as Copilot,
        // to return useful metadata without credentials.
        return factory.list_models(config, provider_key, options).await;
    }

    let mut catalog = ModelCatalog::default();
    let mut keys = self.factories.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    for provider_key in keys {
        let factory = self.factories.get(&provider_key).expect("factory exists");
        if !options.include_unavailable
            && factory.requires_credentials(&provider_key)
            && !has_credentials(config, &provider_key)
        {
            continue;
        }
        match factory.list_models(config, &provider_key, options).await {
            Ok(provider_catalog) => catalog.extend(provider_catalog),
            Err(RociError::MissingCredential { .. } | RociError::MissingConfiguration { .. })
                if !options.include_unavailable => {}
            Err(error) => return Err(error),
        }
    }
    Ok(catalog)
}
```

Also make `provider_keys()` deterministic:

```rust
pub fn provider_keys(&self) -> Vec<&str> {
    let mut keys = self.factories.keys().map(|s| s.as_str()).collect::<Vec<_>>();
    keys.sort();
    keys
}
```

- [ ] **Step 7: Run core tests**

Run:

```bash
cargo test -p roci-core catalog
cargo test -p roci-core registry
```

Expected: pass.

---

### Task 2: Built-In Static Provider Catalogs (`tsq-r0c1m0d5.2`)

**Files:**
- Create: `crates/roci-providers/src/models/catalog.rs`
- Modify: `crates/roci-providers/src/models/mod.rs`
- Modify: `crates/roci-providers/src/factories.rs`
- Modify: `crates/roci-providers/src/overflow.rs`

- [ ] **Step 1: Write provider catalog tests**

Create tests in `crates/roci-providers/src/models/catalog.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "openai")]
    #[test]
    fn openai_static_catalog_contains_gpt4o_with_capabilities() {
        let catalog = openai_catalog("openai");
        let gpt4o = catalog
            .models()
            .iter()
            .find(|model| model.model_id == "gpt-4o")
            .expect("gpt-4o present");

        assert_eq!(gpt4o.provider_key, "openai");
        assert!(gpt4o.capabilities.supports_vision);
        assert!(gpt4o.capabilities.supports_tools);
        assert!(gpt4o.policy.requires_credentials);
        assert!(!gpt4o.policy.local);
    }

    #[cfg(feature = "google")]
    #[test]
    fn google_static_catalog_contains_gemini_with_vision() {
        let catalog = google_catalog("google");
        let gemini = catalog
            .models()
            .iter()
            .find(|model| model.model_id == "gemini-2.5-pro")
            .expect("gemini present");

        assert!(gemini.capabilities.supports_vision);
        assert_eq!(gemini.provider_key, "google");
    }

    #[cfg(feature = "ollama")]
    #[test]
    fn ollama_static_catalog_is_local() {
        let catalog = ollama_catalog("ollama");
        let llama = catalog
            .models()
            .iter()
            .find(|model| model.model_id == "llama3.3")
            .expect("llama3.3 present");

        assert!(llama.policy.local);
        assert!(!llama.policy.requires_credentials);
    }

    #[cfg(feature = "all-providers")]
    #[test]
    fn all_provider_catalog_builders_do_not_panic() {
        assert!(!openai_catalog("openai").models().is_empty());
        assert!(!codex_catalog("codex").models().is_empty());
        assert!(!grok_catalog("grok").models().is_empty());
        assert!(!groq_catalog("groq").models().is_empty());
        assert!(!mistral_catalog("mistral").models().is_empty());
        assert!(!ollama_catalog("ollama").models().is_empty());
        assert!(lmstudio_catalog("lmstudio").models().is_empty());
    }
}
```

- [ ] **Step 2: Run failing provider catalog tests**

Run:

```bash
cargo test -p roci-providers catalog
```

Expected before implementation: compile failure for missing catalog module/functions.

- [ ] **Step 3: Implement static catalog helpers**

Create `crates/roci-providers/src/models/catalog.rs` with helpers:

```rust
use std::collections::BTreeMap;

use roci_core::models::{
    ModelCapabilities, ModelCatalog, ModelCatalogSource, ModelInfo, ModelPolicy,
};

fn model_info(
    provider_key: &str,
    model_id: &str,
    capabilities: ModelCapabilities,
    requires_credentials: bool,
    local: bool,
    default_for_provider: bool,
) -> ModelInfo {
    ModelInfo {
        provider_key: provider_key.to_string(),
        model_id: model_id.to_string(),
        display_name: Some(model_id.to_string()),
        capabilities,
        policy: ModelPolicy {
            requires_credentials,
            local,
            deprecated: false,
            default_for_provider,
        },
        source: ModelCatalogSource::Static,
        metadata: BTreeMap::new(),
    }
}
```

Add provider functions behind feature flags:

```rust
#[cfg(feature = "openai")]
pub fn openai_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::openai::OpenAiModel;

    let models = [
        OpenAiModel::Gpt4o,
        OpenAiModel::Gpt4oMini,
        OpenAiModel::Gpt41,
        OpenAiModel::Gpt41Mini,
        OpenAiModel::Gpt5,
        OpenAiModel::Gpt51,
        OpenAiModel::Gpt52,
        OpenAiModel::Gpt5Mini,
        OpenAiModel::Gpt5Nano,
        OpenAiModel::O3,
        OpenAiModel::O3Mini,
        OpenAiModel::O4Mini,
    ];
    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(
            provider_key,
            &id,
            model.capabilities(),
            true,
            false,
            id == "gpt-4o",
        )
    }))
}

#[cfg(feature = "openai")]
pub fn codex_catalog(provider_key: &str) -> ModelCatalog {
    let mut catalog = openai_catalog(provider_key);
    catalog.update_models(|model| {
        model.provider_key = provider_key.to_string();
        model.policy.default_for_provider = model.model_id == "gpt-5";
    });
    catalog
}

#[cfg(feature = "anthropic")]
pub fn anthropic_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::anthropic::AnthropicModel;

    let models = [
        AnthropicModel::ClaudeOpus45,
        AnthropicModel::ClaudeSonnet45,
        AnthropicModel::ClaudeHaiku35,
    ];
    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(provider_key, &id, model.capabilities(), true, false, false)
    }))
}

#[cfg(feature = "google")]
pub fn google_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::google::GoogleModel;
    let models = [
        GoogleModel::Gemini25Pro,
        GoogleModel::Gemini25Flash,
        GoogleModel::Gemini25FlashLite,
        GoogleModel::Gemini20Flash,
        GoogleModel::Gemini3Flash,
        GoogleModel::Gemini3FlashPreview,
        GoogleModel::Gemini3ProPreview,
        GoogleModel::Gemini15Pro,
        GoogleModel::Gemini15Flash,
    ];
    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(provider_key, &id, model.capabilities(), true, false, id == "gemini-2.5-pro")
    }))
}
```

Add concrete builders for current fixed enums:

```rust
#[cfg(feature = "grok")]
pub fn grok_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::grok::GrokModel;
    let models = [GrokModel::Grok3, GrokModel::Grok3Mini, GrokModel::Grok4, GrokModel::Grok41Fast];
    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(provider_key, &id, model.capabilities(), true, false, false)
    }))
}

#[cfg(feature = "groq")]
pub fn groq_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::groq::GroqModel;
    let models = [
        GroqModel::Llama3370bVersatile,
        GroqModel::Llama318bInstant,
        GroqModel::Mixtral8x7b,
    ];
    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(provider_key, &id, model.capabilities(), true, false, false)
    }))
}

#[cfg(feature = "mistral")]
pub fn mistral_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::mistral::MistralModel;
    let models = [
        MistralModel::MistralLarge,
        MistralModel::MistralMedium,
        MistralModel::MistralSmall,
        MistralModel::Codestral,
    ];
    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(provider_key, &id, model.capabilities(), true, false, false)
    }))
}

#[cfg(feature = "ollama")]
pub fn ollama_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::ollama::OllamaModel;
    let models = [
        OllamaModel::Llama33,
        OllamaModel::Llama31,
        OllamaModel::Mistral,
        OllamaModel::CodeLlama,
        OllamaModel::DeepseekR1,
        OllamaModel::Qwen25,
    ];
    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(provider_key, &id, model.capabilities(), false, true, id == "llama3.3")
    }))
}

#[cfg(feature = "lmstudio")]
pub fn lmstudio_catalog(_provider_key: &str) -> ModelCatalog {
    ModelCatalog::default()
}
```

Use exact enum variant names from current provider files. Keep compatible/router
providers empty unless current code has fixed model enums.

Export module from `crates/roci-providers/src/models/mod.rs`:

```rust
pub mod catalog;
```

- [ ] **Step 4: Wire factories to static catalogs**

In `crates/roci-providers/src/factories.rs`, import:

```rust
use futures::future::BoxFuture;
use roci_core::models::{ModelCatalog, ModelListOptions};
```

For each factory, add:

```rust
fn list_models<'a>(
    &'a self,
    _config: &'a RociConfig,
    provider_key: &'a str,
    _options: &'a ModelListOptions,
) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
    Box::pin(async move { Ok(crate::models::catalog::openai_catalog(provider_key)) })
}
```

For `OllamaFactory` and `LmStudioFactory`, keep `requires_credentials=false` and return empty static catalog unless current model enum has fixed built-ins worth listing.
Wire every built-in factory to its matching builder:

```text
OpenAiFactory -> openai_catalog
CodexFactory -> codex_catalog
AnthropicFactory -> anthropic_catalog
GoogleFactory -> google_catalog
GrokFactory -> grok_catalog
GroqFactory -> groq_catalog
MistralFactory -> mistral_catalog
OllamaFactory -> ollama_catalog
LmStudioFactory -> lmstudio_catalog
OpenAiCompatibleFactory/OpenRouterFactory/TogetherFactory/AnthropicCompatibleFactory/AzureFactory -> empty catalog unless a current fixed enum exists
GitHubCopilotFactory -> handled in Task 3 after static fallback exists
```

- [ ] **Step 5: Delegate through overflow wrapper**

Modify `crates/roci-providers/src/overflow.rs` `impl ProviderFactory for OverflowClassifyingFactory`:

```rust
fn list_models<'a>(
    &'a self,
    config: &'a RociConfig,
    provider_key: &'a str,
    options: &'a ModelListOptions,
) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
    self.inner.list_models(config, provider_key, options)
}
```

Add imports for `BoxFuture`, `RociConfig`, `ModelCatalog`, and `ModelListOptions` if not already present.

- [ ] **Step 6: Run provider tests**

Run:

```bash
cargo test -p roci-providers catalog
cargo test -p roci-providers factory_registration
cargo test -p roci-providers --features all-providers catalog
```

Expected: pass.

---

### Task 3: GitHub Copilot Dynamic Discovery (`tsq-r0c1m0d5.3`)

**Files:**
- Modify: `crates/roci-providers/src/provider/github_copilot.rs`
- Modify: `crates/roci-providers/src/factories.rs`
- Modify: `crates/roci-providers/src/models/catalog.rs`

- [ ] **Step 1: Write Copilot parser tests**

In `crates/roci-providers/src/provider/github_copilot.rs`, add tests:

```rust
#[cfg(test)]
mod model_tests {
    use super::*;

    #[test]
    fn parse_models_accepts_openai_style_data_wrapper() {
        let body = r#"{"data":[{"id":"gpt-4.1","object":"model"},{"id":"claude-sonnet-4","owned_by":"github"}]}"#;

        let models = parse_copilot_models_response(body).unwrap();

        assert_eq!(models, vec!["gpt-4.1", "claude-sonnet-4"]);
    }

    #[test]
    fn parse_models_accepts_raw_array() {
        let body = r#"[{"id":"gpt-5-mini"},{"id":"o4-mini"}]"#;

        let models = parse_copilot_models_response(body).unwrap();

        assert_eq!(models, vec!["gpt-5-mini", "o4-mini"]);
    }

    #[test]
    fn parse_models_rejects_2xx_without_ids() {
        let err = parse_copilot_models_response(r#"{"data":[{"name":"missing"}]}"#).unwrap_err();

        assert!(matches!(err, RociError::Provider { provider, .. } if provider == "github-copilot"));
    }
}
```

- [ ] **Step 2: Add mock dynamic discovery tests**

Use `wiremock` in the same module:

```rust
#[tokio::test]
async fn list_models_fetches_dynamic_models() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"data":[{"id":"gpt-4.1"}]}"#,
            "application/json",
        ))
        .mount(&server)
        .await;

    let catalog = list_copilot_models("test-token", &server.uri(), "github-copilot")
        .await
        .unwrap();

    assert_eq!(catalog.models()[0].model_id, "gpt-4.1");
    assert!(matches!(catalog.models()[0].source, ModelCatalogSource::Dynamic { .. }));
}

#[tokio::test]
async fn list_models_404_uses_static_fallback() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let err = list_copilot_models("test-token", &server.uri(), "github-copilot")
        .await
        .unwrap_err();

    assert!(matches!(err, RociError::UnsupportedOperation(_)));
}

#[tokio::test]
async fn list_models_5xx_is_api_error_for_factory_fallback() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
        .mount(&server)
        .await;

    let err = list_copilot_models("test-token", &server.uri(), "github-copilot")
        .await
        .unwrap_err();

    assert!(matches!(err, RociError::Api { status: 503, .. }));
}
```

- [ ] **Step 2b: Add Copilot factory fallback tests**

Add tests in `crates/roci-providers/src/factories.rs` under `#[cfg(all(test, feature = "github-copilot"))]`:

```rust
#[tokio::test]
async fn copilot_list_models_missing_creds_returns_static_fallback() {
    let config = RociConfig::new().with_token_store(None);
    let options = ModelListOptions {
        provider_key: Some("github-copilot".to_string()),
        ..ModelListOptions::default()
    };

    let catalog = GitHubCopilotFactory
        .list_models(&config, "github-copilot", &options)
        .await
        .unwrap();

    assert!(!catalog.models().is_empty());
    assert!(catalog.models().iter().all(|model| matches!(model.source, ModelCatalogSource::Static)));
}

#[tokio::test]
async fn copilot_list_models_include_dynamic_false_skips_http() {
    let config = RociConfig::new().with_token_store(None);
    let options = ModelListOptions {
        include_dynamic: false,
        ..ModelListOptions::default()
    };

    let catalog = GitHubCopilotFactory
        .list_models(&config, "github-copilot", &options)
        .await
        .unwrap();

    assert!(!catalog.models().is_empty());
}

#[tokio::test]
async fn copilot_list_models_include_static_false_requires_credentials() {
    let config = RociConfig::new().with_token_store(None);
    let options = ModelListOptions {
        include_static: false,
        ..ModelListOptions::default()
    };

    let err = GitHubCopilotFactory
        .list_models(&config, "github-copilot", &options)
        .await
        .unwrap_err();

    assert!(matches!(err, RociError::MissingCredential { .. }));
}
```

Add wiremock-backed factory tests for 404/405 static fallback, 401/403 auth error
without fallback, and 5xx/timeout fallback with `metadata["warning"]`. Configure
the factory through `RociConfig::set_api_key("github-copilot", "...")` and
`RociConfig::set_base_url("github-copilot", server.uri())`.

- [ ] **Step 3: Run failing Copilot tests**

Run:

```bash
cargo test -p roci-providers --features github-copilot github_copilot
```

Expected before implementation: compile failure for parser/client helpers.

- [ ] **Step 4: Implement Copilot parser and client helper**

Add to `crates/roci-providers/src/provider/github_copilot.rs`:

```rust
use roci_core::models::{ModelCatalog, ModelCatalogSource, ModelInfo, ModelPolicy};

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum ModelsEnvelope {
    Data { data: Vec<ModelEntry> },
    Array(Vec<ModelEntry>),
}

#[derive(serde::Deserialize)]
struct ModelEntry {
    id: Option<String>,
}

pub(crate) fn parse_copilot_models_response(body: &str) -> Result<Vec<String>, RociError> {
    let envelope: ModelsEnvelope = serde_json::from_str(body).map_err(|err| {
        RociError::Provider {
            provider: "github-copilot".to_string(),
            message: format!("failed to parse models response: {err}"),
        }
    })?;
    let entries = match envelope {
        ModelsEnvelope::Data { data } => data,
        ModelsEnvelope::Array(data) => data,
    };
    let ids = entries
        .into_iter()
        .filter_map(|entry| entry.id)
        .filter(|id| !id.trim().is_empty())
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Err(RociError::Provider {
            provider: "github-copilot".to_string(),
            message: "models response did not contain model ids".to_string(),
        });
    }
    Ok(ids)
}
```

Add async client:

```rust
pub(crate) async fn list_copilot_models(
    api_key: &str,
    base_url: &str,
    provider_key: &str,
) -> Result<ModelCatalog, RociError> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .get(&url)
        .bearer_auth(api_key)
        .headers(copilot_headers())
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;

    match status.as_u16() {
        200..=299 => {
            let ids = parse_copilot_models_response(&body)?;
            Ok(catalog_from_copilot_ids(provider_key, ids, ModelCatalogSource::Dynamic {
                endpoint: "/models".to_string(),
            }))
        }
        401 | 403 => Err(RociError::Authentication(
            "github-copilot models listing rejected credentials".to_string(),
        )),
        404 | 405 => Err(RociError::UnsupportedOperation(
            "github-copilot models endpoint is unavailable".to_string(),
        )),
        500..=599 => Err(RociError::api(status.as_u16(), body)),
        _ => Err(RociError::api(status.as_u16(), body)),
    }
}
```

Extract current header construction into `fn copilot_headers() -> HeaderMap` and use it in both `GitHubCopilotProvider::new` and `list_copilot_models`.

- [ ] **Step 5: Add Copilot static fallback catalog**

In `crates/roci-providers/src/models/catalog.rs`:

```rust
#[cfg(feature = "github-copilot")]
pub fn github_copilot_static_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::openai::OpenAiModel;

    let models = [
        OpenAiModel::Gpt41,
        OpenAiModel::Gpt41Mini,
        OpenAiModel::Gpt5,
        OpenAiModel::Gpt5Mini,
        OpenAiModel::O4Mini,
    ];
    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(provider_key, &id, model.capabilities(), true, false, false)
    }))
}
```

In Copilot client module, share `catalog_from_copilot_ids`:

```rust
fn catalog_from_copilot_ids(
    provider_key: &str,
    ids: Vec<String>,
    source: ModelCatalogSource,
) -> ModelCatalog {
    ModelCatalog::from_models(ids.into_iter().map(|id| ModelInfo {
        provider_key: provider_key.to_string(),
        model_id: id.clone(),
        display_name: Some(id),
        capabilities: roci_core::models::ModelCapabilities::default(),
        policy: ModelPolicy {
            requires_credentials: true,
            local: false,
            deprecated: false,
            default_for_provider: false,
        },
        source: source.clone(),
        metadata: Default::default(),
    }))
}
```

- [ ] **Step 6: Wire Copilot factory listing with fallback matrix**

In `GitHubCopilotFactory::list_models`:

```rust
fn list_models<'a>(
    &'a self,
    config: &'a RociConfig,
    provider_key: &'a str,
    options: &'a ModelListOptions,
) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
    Box::pin(async move {
        let static_catalog = || crate::models::catalog::github_copilot_static_catalog(provider_key);

        let Some((api_key, base_url)) = resolve_copilot_model_list_credentials(config)? else {
            return if options.include_static {
                Ok(static_catalog())
            } else {
                Err(RociError::MissingCredential {
                    provider: provider_key.to_string(),
                })
            };
        };
        if !options.include_dynamic {
            return Ok(static_catalog());
        }

        match crate::provider::github_copilot::list_copilot_models(&api_key, &base_url, provider_key).await {
            Ok(catalog) => Ok(catalog),
            Err(RociError::UnsupportedOperation(_)) if options.include_static => Ok(static_catalog()),
            Err(RociError::Api { status, .. }) if (500..=599).contains(&status) && options.include_static => {
                let mut catalog = static_catalog();
                catalog.update_models(|model| {
                    model.metadata.insert(
                        "warning".to_string(),
                        serde_json::Value::String("github-copilot dynamic model discovery unavailable".to_string()),
                    );
                });
                Ok(catalog)
            }
            Err(RociError::Network(err)) if err.is_timeout() && options.include_static => {
                let mut catalog = static_catalog();
                catalog.update_models(|model| {
                    model.metadata.insert(
                        "warning".to_string(),
                        serde_json::Value::String("github-copilot dynamic model discovery timed out".to_string()),
                    );
                });
                Ok(catalog)
            }
            Err(RociError::Timeout(_)) if options.include_static => {
                let mut catalog = static_catalog();
                catalog.update_models(|model| {
                    model.metadata.insert(
                        "warning".to_string(),
                        serde_json::Value::String("github-copilot dynamic model discovery timed out".to_string()),
                    );
                });
                Ok(catalog)
            }
            Err(error) => Err(error),
        }
    })
}
```

`resolve_copilot_model_list_credentials` should reuse the same credential source
as `GitHubCopilotFactory::create`: prefer valid `github-copilot-api` token store
entry and use its `account_id` as base URL, then fall back to config API key and
base URL.

If `ModelCatalog` lacks `update_models`, add it in Task 1:

```rust
pub fn update_models(&mut self, mut update: impl FnMut(&mut ModelInfo)) {
    for model in &mut self.models {
        update(model);
    }
    self.normalize();
}
```

- [ ] **Step 7: Run Copilot tests**

Run:

```bash
cargo test -p roci-providers --features github-copilot github_copilot
cargo test -p roci-providers --features github-copilot catalog
```

Expected: pass.

---

### Task 4: Runtime Current/Switch Surface (`tsq-r0c1m0d5.5`)

**Files:**
- Modify: `crates/roci-core/src/agent/runtime/mutations.rs`
- Modify: `crates/roci-core/src/agent/runtime_tests/state_lifecycle.rs`

- [ ] **Step 1: Write runtime switching tests**

Add to `crates/roci-core/src/agent/runtime_tests/state_lifecycle.rs`:

```rust
#[tokio::test]
async fn current_model_returns_runtime_model() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    let model = agent.current_model().await;

    assert_eq!(model, test_agent_config().model);
}

#[tokio::test]
async fn switch_model_returns_previous_model_and_updates_current() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    let previous = agent.current_model().await;
    let next: LanguageModel = "openai:gpt-4o-mini".parse().unwrap();

    let returned = agent.switch_model(next.clone()).await.unwrap();

    assert_eq!(returned, previous);
    assert_eq!(agent.current_model().await, next);
}

#[tokio::test]
async fn switch_model_rejects_when_running() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    *agent.state.lock().await = AgentState::Running;
    let next: LanguageModel = "openai:gpt-4o-mini".parse().unwrap();

    let err = agent.switch_model(next).await.unwrap_err();

    assert!(matches!(err, RociError::InvalidState(_)));
}
```

- [ ] **Step 2: Run failing runtime tests**

Run:

```bash
cargo test -p roci-core --features agent state_lifecycle
```

Expected before implementation: compile failure for `current_model`/`switch_model`.

- [ ] **Step 3: Implement runtime APIs**

Modify `crates/roci-core/src/agent/runtime/mutations.rs`:

```rust
/// Return the model used for subsequent runs.
pub async fn current_model(&self) -> LanguageModel {
    self.model.lock().await.clone()
}

/// Replace the configured model used for subsequent runs, returning the previous model.
///
/// # Errors
///
/// Returns [`RociError::InvalidState`] if the runtime is not idle.
pub async fn switch_model(&self, model: LanguageModel) -> Result<LanguageModel, RociError> {
    let _state_guard = self.lock_state_for_idle_mutation()?;
    let mut runtime_model = self
        .model
        .try_lock()
        .map_err(|_| RociError::InvalidState("Agent is busy (model lock contended)".into()))?;
    let previous = runtime_model.clone();
    *runtime_model = model;
    Ok(previous)
}

/// Replace the configured model used for subsequent runs.
///
/// # Errors
///
/// Returns [`RociError::InvalidState`] if the runtime is not idle.
pub async fn set_model(&self, model: LanguageModel) -> Result<(), RociError> {
    self.switch_model(model).await.map(|_| ())
}
```

Remove the old duplicated `set_model` body.

- [ ] **Step 4: Run runtime tests**

Run:

```bash
cargo test -p roci-core --features agent state_lifecycle
```

Expected: pass.

---

### Task 5: `roci-agent models list` CLI Harness (`tsq-r0c1m0d5.4`)

**Files:**
- Modify: `crates/roci-cli/src/cli/mod.rs`
- Create: `crates/roci-cli/src/models_cmd.rs`
- Modify: `crates/roci-cli/src/main.rs`

- [ ] **Step 1: Add CLI parse tests**

In `crates/roci-cli/src/cli/mod.rs` tests:

```rust
#[test]
fn parse_models_list_defaults() {
    let cli = Cli::try_parse_from(["roci-agent", "models", "list"]).unwrap();
    match cli.command {
        Commands::Models(args) => match args.command {
            ModelsCommands::List(args) => {
                assert!(args.provider.is_none());
                assert!(!args.json);
            }
        },
        other => panic!("expected Models, got {other:?}"),
    }
}

#[test]
fn parse_models_list_provider_json() {
    let cli = Cli::try_parse_from([
        "roci-agent",
        "models",
        "list",
        "--provider",
        "openai",
        "--json",
    ])
    .unwrap();
    match cli.command {
        Commands::Models(args) => match args.command {
            ModelsCommands::List(args) => {
                assert_eq!(args.provider.as_deref(), Some("openai"));
                assert!(args.json);
            }
        },
        other => panic!("expected Models, got {other:?}"),
    }
}
```

- [ ] **Step 2: Add CLI command types**

Modify `crates/roci-cli/src/cli/mod.rs`:

```rust
pub enum Commands {
    Auth(AuthArgs),
    Audio(AudioArgs),
    Chat(ChatArgs),
    Models(ModelsArgs),
    Session(SessionArgs),
    Skills(SkillsArgs),
}

#[derive(Parser, Debug)]
pub struct ModelsArgs {
    #[command(subcommand)]
    pub command: ModelsCommands,
}

#[derive(Subcommand, Debug)]
pub enum ModelsCommands {
    /// List available models
    List(ModelsListArgs),
}

#[derive(Parser, Debug)]
pub struct ModelsListArgs {
    /// Provider key to list, for example openai or github-copilot
    #[arg(long)]
    pub provider: Option<String>,

    /// Print models as JSON
    #[arg(long)]
    pub json: bool,
}
```

- [ ] **Step 3: Write injectable command tests**

Create `crates/roci-cli/src/models_cmd.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use futures::future::BoxFuture;
    use roci::config::RociConfig;
    use roci::models::{ModelCatalog, ModelCatalogSource, ModelInfo, ModelListOptions, ModelPolicy};
    use roci::provider::{ModelProvider, ProviderFactory};

    struct RecordingFactory {
        calls: Arc<AtomicUsize>,
    }

    impl ProviderFactory for RecordingFactory {
        fn provider_keys(&self) -> &[&str] {
            &["sentinel"]
        }

        fn requires_credentials(&self, _provider_key: &str) -> bool {
            false
        }

        fn list_models<'a>(
            &'a self,
            _config: &'a RociConfig,
            provider_key: &'a str,
            _options: &'a ModelListOptions,
        ) -> BoxFuture<'a, Result<ModelCatalog, roci::error::RociError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(ModelCatalog::from_models([ModelInfo {
                    provider_key: provider_key.to_string(),
                    model_id: "sentinel-model".to_string(),
                    display_name: None,
                    capabilities: Default::default(),
                    policy: ModelPolicy {
                        requires_credentials: false,
                        local: true,
                        deprecated: false,
                        default_for_provider: true,
                    },
                    source: ModelCatalogSource::Static,
                    metadata: Default::default(),
                }]))
            })
        }

        fn create(
            &self,
            _config: &RociConfig,
            _provider_key: &str,
            _model_id: &str,
        ) -> Result<Box<dyn ModelProvider>, roci::error::RociError> {
            unreachable!("models list must not create providers")
        }
    }

    #[tokio::test]
    async fn json_output_uses_real_registry_catalog() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = roci::provider::ProviderRegistry::new();
        registry.register(Arc::new(RecordingFactory {
            calls: calls.clone(),
        }));
        let mut output = Vec::new();
        let args = ModelsListArgs {
            provider: Some("sentinel".to_string()),
            json: true,
        };

        run_list(args, Arc::new(registry), RociConfig::new().with_token_store(None), &mut output)
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(json["models"][0]["model_id"], "sentinel-model");
    }

    #[tokio::test]
    async fn human_output_contains_registry_model_rows() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = roci::provider::ProviderRegistry::new();
        registry.register(Arc::new(RecordingFactory {
            calls: calls.clone(),
        }));
        let mut output = Vec::new();
        let args = ModelsListArgs {
            provider: Some("sentinel".to_string()),
            json: false,
        };

        run_list(args, Arc::new(registry), RociConfig::new().with_token_store(None), &mut output)
            .await
            .unwrap();

        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("PROVIDER"));
        assert!(text.contains("sentinel-model"));
    }

    #[tokio::test]
    async fn explicit_unavailable_provider_error_reaches_cli_runner() {
        let registry = roci::provider::ProviderRegistry::new();
        let mut output = Vec::new();
        let args = ModelsListArgs {
            provider: Some("missing".to_string()),
            json: true,
        };

        let err = run_list(
            args,
            Arc::new(registry),
            RociConfig::new().with_token_store(None),
            &mut output,
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("missing"));
    }
}
```

- [ ] **Step 4: Implement `models_cmd` runner and rendering**

Create `crates/roci-cli/src/models_cmd.rs`:

```rust
use std::io::Write;
use std::sync::Arc;

use roci::config::RociConfig;
use roci::models::{ModelCatalogSource, ModelListOptions};
use roci::provider::ProviderRegistry;

use crate::cli::{ModelsArgs, ModelsCommands, ModelsListArgs};

pub async fn handle_models(args: ModelsArgs) -> Result<(), Box<dyn std::error::Error>> {
    match args.command {
        ModelsCommands::List(args) => {
            let registry = Arc::new(roci::default_registry());
            let config = RociConfig::from_env();
            let mut stdout = std::io::stdout();
            run_list(args, registry, config, &mut stdout).await?;
        }
    }
    Ok(())
}

pub async fn run_list<W: Write>(
    args: ModelsListArgs,
    registry: Arc<ProviderRegistry>,
    config: RociConfig,
    writer: &mut W,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = ModelListOptions {
        provider_key: args.provider.clone(),
        ..ModelListOptions::default()
    };
    let catalog = registry.list_models(&config, &options).await?;
    if args.json {
        let output = serde_json::json!({
            "models": catalog.into_models(),
        });
        writeln!(writer, "{}", serde_json::to_string_pretty(&output)?)?;
    } else {
        writeln!(writer, "PROVIDER        MODEL                         CONTEXT   TOOLS   VISION   SOURCE")?;
        for model in catalog.models() {
            writeln!(
                writer,
                "{:<15} {:<29} {:<9} {:<7} {:<8} {}",
                model.provider_key,
                model.model_id,
                model.capabilities.context_length,
                yes_no(model.capabilities.supports_tools),
                yes_no(model.capabilities.supports_vision),
                source_label(&model.source),
            )?;
        }
    }
    Ok(())
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn source_label(source: &ModelCatalogSource) -> &'static str {
    match source {
        ModelCatalogSource::Static => "static",
        ModelCatalogSource::Dynamic { .. } => "dynamic",
    }
}
```

- [ ] **Step 5: Wire main**

Modify `crates/roci-cli/src/main.rs`:

```rust
mod models_cmd;
```

and command dispatch:

```rust
Commands::Models(models_args) => models_cmd::handle_models(models_args).await,
```

- [ ] **Step 6: Run CLI tests**

Run:

```bash
cargo test -p roci-cli models
```

Expected: pass.

---

### Task 6: Docs And Testing Updates (`tsq-r0c1m0d5.6` docs slice)

**Files:**
- Modify: `docs/models.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/testing.md`
- Optional modify: `docs/architecture/providers-soc.md`

- [ ] **Step 1: Update `docs/models.md`**

Add section:

```markdown
## Model Catalog

`roci-core` exposes provider-neutral catalog types in
`roci_core::models::catalog`:

- `ModelInfo`
- `ModelPolicy`
- `ModelCatalogSource`
- `ModelListOptions`
- `ModelCatalog`

Built-in providers expose static catalogs from existing model enums and
capability methods. Dynamic listing is provider-specific; GitHub Copilot attempts
`/models` when authenticated and falls back to static entries for explicit
provider listing when the endpoint is unavailable.
```

- [ ] **Step 2: Update architecture docs**

In `docs/ARCHITECTURE.md` and `docs/architecture/providers-soc.md`, add that
`ProviderFactory` owns both provider creation and catalog listing:

```markdown
`ProviderFactory::list_models` returns provider-neutral model metadata without
creating a provider instance. `ProviderRegistry::list_models` aggregates those
catalogs for host apps and CLI tools.
```

- [ ] **Step 3: Update `docs/testing.md`**

Add a model catalog smoke section:

```markdown
## Model Catalog Smoke

Provider-facing model catalog work must be proven through `roci-agent`, not only
unit tests. Run live smokes inside tmux and print the attach command before the
provider-facing command.

Static registry smoke:

```bash
cargo run -q -p roci-cli -- models list --provider openai --json
```

Copilot dynamic smoke, when authenticated:

```bash
cargo run -q -p roci-cli --features roci/github-copilot -- models list --provider github-copilot --json
```
```

- [ ] **Step 4: Run docs grep sanity**

Run:

```bash
rg -n "models list|ModelCatalog|list_models|current_model|switch_model" docs
```

Expected: new docs mention all public surfaces.

---

### Task 7: Integration, Automated Gates, And Live Verification

**Files:**
- No planned source edits unless integration failures require focused fixes.

- [ ] **Step 1: Run formatting**

Run:

```bash
cargo fmt --all -- --check
```

Expected: pass.

- [ ] **Step 2: Run targeted tests**

Run:

```bash
cargo test -p roci-core catalog
cargo test -p roci-core registry
cargo test -p roci-core --features agent state_lifecycle
cargo test -p roci-providers catalog
cargo test -p roci-providers --features github-copilot github_copilot
cargo test -p roci-cli models
```

Expected: pass.

- [ ] **Step 3: Run full Rust gates**

Run:

```bash
cargo clippy --workspace --all-targets --features full -- -D warnings
cargo test --workspace --all-targets
git diff --check
```

Expected: pass.

- [ ] **Step 4: Build CLI used for live smoke**

Run:

```bash
cargo build -p roci-cli --features roci/github-copilot
```

Expected: pass and produce `target/debug/roci-agent`.

- [ ] **Step 5: Run static model-list smoke in tmux**

Show user:

```bash
tmux attach -t roci-models-static
```

Run:

```bash
tmux new-session -d -s roci-models-static 'set -o pipefail; cd /Users/adityasharma/Projects/roci && ./target/debug/roci-agent models list --provider openai --json | tee /tmp/roci-models-static.log; code=$?; echo "[roci-agent models openai exit=$code]" | tee -a /tmp/roci-models-static.log; exit $code'
```

Expected evidence:

```bash
rg -n '"models"|"provider_key": "openai"|"model_id": "gpt-4o"|exit=0' /tmp/roci-models-static.log
```

- [ ] **Step 6: Run Copilot model-list smoke when authenticated**

Check auth/config without printing secrets:

```bash
./target/debug/roci-agent auth status
```

If Copilot authenticated, show user:

```bash
tmux attach -t roci-models-copilot
```

Run:

```bash
tmux new-session -d -s roci-models-copilot 'set -o pipefail; cd /Users/adityasharma/Projects/roci && ./target/debug/roci-agent models list --provider github-copilot --json | tee /tmp/roci-models-copilot.log; code=$?; echo "[roci-agent models copilot exit=$code]" | tee -a /tmp/roci-models-copilot.log; exit $code'
```

Expected evidence:

```bash
rg -n '"models"|"provider_key": "github-copilot"|exit=0' /tmp/roci-models-copilot.log
```

If Copilot auth unavailable, record that dynamic smoke was unavailable and run:

```bash
tmux attach -t roci-models-all
```

Run:

```bash
tmux new-session -d -s roci-models-all 'set -o pipefail; cd /Users/adityasharma/Projects/roci && ./target/debug/roci-agent models list --json | tee /tmp/roci-models-all.log; code=$?; echo "[roci-agent models all exit=$code]" | tee -a /tmp/roci-models-all.log; exit $code'
rg -n '"models"' /tmp/roci-models-all.log
```

- [ ] **Step 7: Update tsq with evidence**

For each completed child:

```bash
tsq done tsq-r0c1m0d5.N --note "<short evidence>"
```

Close parent only after all child tasks pass and live evidence exists:

```bash
tsq done tsq-r0c1m0d5 --note "<automated gates + live tmux evidence>"
```

---

## Self-Review Notes

- Spec coverage: core DTOs/options/dedupe, provider static catalogs, Copilot dynamic/fallback, CLI real registry harness, runtime switch, docs, and live tmux verification all have tasks.
- TDD shape: tasks start with failing tests where behavior changes.
- Parallel shape: Task 1 is the contract gate. Task 2 provider static catalog runs before Task 3 Copilot dynamic work because they share provider catalog/factory files. Task 4 runtime switching and Task 5 CLI parser/stub harness can run after Task 1 with disjoint ownership. Task 7 integrates and verifies.
- Known risk: exact provider enum variant names may differ from examples. Implementers must use current enum names from `crates/roci-providers/src/models/*.rs`, not invent aliases.
