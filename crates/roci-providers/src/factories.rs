//! ProviderFactory implementations for each built-in provider.

use futures::future::BoxFuture;
use roci_core::config::RociConfig;
use roci_core::error::RociError;
use roci_core::models::{ModelCatalog, ModelListOptions, ProviderKey};
use roci_core::provider::{ModelProvider, ProviderFactory};

fn catalog_future<'a>(
    provider_key: &'a str,
    options: &'a ModelListOptions,
    builder: fn(&str) -> ModelCatalog,
) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
    Box::pin(async move {
        if options.include_static {
            Ok(builder(provider_key))
        } else {
            Ok(ModelCatalog::default())
        }
    })
}

/// Resolve an API key from config for the given provider.
fn require_api_key(
    config: &RociConfig,
    provider: ProviderKey,
    missing_message: &'static str,
) -> Result<String, RociError> {
    roci_core::provider::require_api_key(config, provider, missing_message)
}

#[cfg_attr(
    not(any(feature = "openai", feature = "anthropic", test)),
    allow(dead_code)
)]
fn optional_api_key_for(config: &RociConfig, provider: ProviderKey) -> String {
    config.get_api_key_for(provider).unwrap_or_default()
}

#[cfg_attr(
    not(any(feature = "openrouter", feature = "together", test)),
    allow(dead_code)
)]
fn optional_api_key(config: &RociConfig, provider: &str) -> String {
    config.get_api_key(provider).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// OpenAI
// ---------------------------------------------------------------------------

#[cfg(feature = "openai")]
pub struct OpenAiFactory;

#[cfg(feature = "openai")]
impl ProviderFactory for OpenAiFactory {
    fn provider_keys(&self) -> &[&str] {
        &["openai"]
    }

    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        catalog_future(
            provider_key,
            options,
            crate::models::catalog::openai_catalog,
        )
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::openai::OpenAiModel;
        use std::str::FromStr;

        let api_key = optional_api_key_for(config, ProviderKey::OpenAi);
        let model =
            OpenAiModel::from_str(model_id).unwrap_or(OpenAiModel::Custom(model_id.to_string()));
        if model.uses_responses_api() {
            Ok(Box::new(
                crate::provider::openai_responses::OpenAiResponsesProvider::new(
                    model,
                    api_key,
                    config.get_base_url_for(ProviderKey::OpenAi),
                    None,
                ),
            ))
        } else {
            Ok(Box::new(crate::provider::openai::OpenAiProvider::new(
                model,
                api_key,
                config.get_base_url_for(ProviderKey::OpenAi),
                None,
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// OpenAI Codex (Codex CLI backend)
// ---------------------------------------------------------------------------

#[cfg(feature = "openai")]
pub struct CodexFactory;

#[cfg(feature = "openai")]
impl ProviderFactory for CodexFactory {
    fn provider_keys(&self) -> &[&str] {
        &["codex"]
    }

    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        catalog_future(provider_key, options, crate::models::catalog::codex_catalog)
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::openai::OpenAiModel;
        use std::str::FromStr;

        let api_key = optional_api_key_for(config, ProviderKey::Codex);
        let base_url = config
            .get_base_url_for(ProviderKey::Codex)
            .or_else(|| Some("https://chatgpt.com/backend-api/codex".to_string()));
        let account_id = config.get_account_id_for(ProviderKey::Codex);
        let model =
            OpenAiModel::from_str(model_id).unwrap_or(OpenAiModel::Custom(model_id.to_string()));
        if model.uses_responses_api() {
            Ok(Box::new(
                crate::provider::openai_responses::OpenAiResponsesProvider::new(
                    model, api_key, base_url, account_id,
                ),
            ))
        } else {
            Ok(Box::new(crate::provider::openai::OpenAiProvider::new(
                model, api_key, base_url, account_id,
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// Anthropic
// ---------------------------------------------------------------------------

#[cfg(feature = "anthropic")]
pub struct AnthropicFactory;

#[cfg(feature = "anthropic")]
impl ProviderFactory for AnthropicFactory {
    fn provider_keys(&self) -> &[&str] {
        &["anthropic"]
    }

    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        catalog_future(
            provider_key,
            options,
            crate::models::catalog::anthropic_catalog,
        )
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::anthropic::AnthropicModel;
        use std::str::FromStr;

        let api_key = optional_api_key_for(config, ProviderKey::Anthropic);
        let model = AnthropicModel::from_str(model_id)
            .unwrap_or(AnthropicModel::Custom(model_id.to_string()));
        Ok(Box::new(
            crate::provider::anthropic::AnthropicProvider::new(
                model,
                api_key,
                config.get_base_url_for(ProviderKey::Anthropic),
            ),
        ))
    }
}

// ---------------------------------------------------------------------------
// Google
// ---------------------------------------------------------------------------

#[cfg(feature = "google")]
pub struct GoogleFactory;

#[cfg(feature = "google")]
impl ProviderFactory for GoogleFactory {
    fn provider_keys(&self) -> &[&str] {
        &["google"]
    }

    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        catalog_future(
            provider_key,
            options,
            crate::models::catalog::google_catalog,
        )
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::google::GoogleModel;
        use std::str::FromStr;

        let api_key = require_api_key(config, ProviderKey::Google, "Missing GOOGLE_API_KEY")?;
        let model =
            GoogleModel::from_str(model_id).unwrap_or(GoogleModel::Custom(model_id.to_string()));
        Ok(Box::new(crate::provider::google::GoogleProvider::new(
            model, api_key,
        )))
    }
}

// ---------------------------------------------------------------------------
// Grok
// ---------------------------------------------------------------------------

#[cfg(feature = "grok")]
pub struct GrokFactory;

#[cfg(feature = "grok")]
impl ProviderFactory for GrokFactory {
    fn provider_keys(&self) -> &[&str] {
        &["grok"]
    }

    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        catalog_future(provider_key, options, crate::models::catalog::grok_catalog)
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::grok::GrokModel;
        use std::str::FromStr;

        let api_key = require_api_key(config, ProviderKey::Grok, "Missing XAI_API_KEY")?;
        let model =
            GrokModel::from_str(model_id).unwrap_or(GrokModel::Custom(model_id.to_string()));
        Ok(Box::new(crate::provider::grok::GrokProvider::new(
            model, api_key,
        )))
    }
}

// ---------------------------------------------------------------------------
// Groq
// ---------------------------------------------------------------------------

#[cfg(feature = "groq")]
pub struct GroqFactory;

#[cfg(feature = "groq")]
impl ProviderFactory for GroqFactory {
    fn provider_keys(&self) -> &[&str] {
        &["groq"]
    }

    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        catalog_future(provider_key, options, crate::models::catalog::groq_catalog)
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::groq::GroqModel;
        use std::str::FromStr;

        let api_key = require_api_key(config, ProviderKey::Groq, "Missing GROQ_API_KEY")?;
        let model =
            GroqModel::from_str(model_id).unwrap_or(GroqModel::Custom(model_id.to_string()));
        Ok(Box::new(crate::provider::groq::GroqProvider::new(
            model, api_key,
        )))
    }
}

// ---------------------------------------------------------------------------
// Mistral
// ---------------------------------------------------------------------------

#[cfg(feature = "mistral")]
pub struct MistralFactory;

#[cfg(feature = "mistral")]
impl ProviderFactory for MistralFactory {
    fn provider_keys(&self) -> &[&str] {
        &["mistral"]
    }

    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        catalog_future(
            provider_key,
            options,
            crate::models::catalog::mistral_catalog,
        )
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::mistral::MistralModel;
        use std::str::FromStr;

        let api_key = require_api_key(config, ProviderKey::Mistral, "Missing MISTRAL_API_KEY")?;
        let model =
            MistralModel::from_str(model_id).unwrap_or(MistralModel::Custom(model_id.to_string()));
        Ok(Box::new(crate::provider::mistral::MistralProvider::new(
            model, api_key,
        )))
    }
}

// ---------------------------------------------------------------------------
// Ollama
// ---------------------------------------------------------------------------

#[cfg(feature = "ollama")]
pub struct OllamaFactory;

#[cfg(feature = "ollama")]
impl ProviderFactory for OllamaFactory {
    fn provider_keys(&self) -> &[&str] {
        &["ollama"]
    }

    fn requires_credentials(&self, _provider_key: &str) -> bool {
        false
    }

    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        catalog_future(
            provider_key,
            options,
            crate::models::catalog::ollama_catalog,
        )
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::ollama::OllamaModel;
        use std::str::FromStr;

        let base_url = config
            .get_base_url_for(ProviderKey::Ollama)
            .unwrap_or_else(|| "http://localhost:11434".to_string());
        let model =
            OllamaModel::from_str(model_id).unwrap_or(OllamaModel::Custom(model_id.to_string()));
        Ok(Box::new(crate::provider::ollama::OllamaProvider::new(
            model, base_url,
        )))
    }
}

// ---------------------------------------------------------------------------
// LMStudio
// ---------------------------------------------------------------------------

#[cfg(feature = "lmstudio")]
pub struct LmStudioFactory;

#[cfg(feature = "lmstudio")]
impl ProviderFactory for LmStudioFactory {
    fn provider_keys(&self) -> &[&str] {
        &["lmstudio"]
    }

    fn requires_credentials(&self, _provider_key: &str) -> bool {
        false
    }

    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        catalog_future(
            provider_key,
            options,
            crate::models::catalog::lmstudio_catalog,
        )
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::lmstudio::LmStudioModel;

        let base_url = config
            .get_base_url_for(ProviderKey::LmStudio)
            .unwrap_or_else(|| "http://localhost:1234".to_string());
        let model = LmStudioModel::Custom(model_id.to_string());
        Ok(Box::new(crate::provider::lmstudio::LmStudioProvider::new(
            model, base_url,
        )))
    }
}

// ---------------------------------------------------------------------------
// OpenAI Compatible
// ---------------------------------------------------------------------------

#[cfg(feature = "openai-compatible")]
pub struct OpenAiCompatibleFactory;

#[cfg(feature = "openai-compatible")]
impl ProviderFactory for OpenAiCompatibleFactory {
    fn provider_keys(&self) -> &[&str] {
        &["openai-compatible"]
    }

    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        catalog_future(provider_key, options, crate::models::catalog::empty_catalog)
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        let api_key = config
            .get_api_key_for(ProviderKey::OpenAiCompatible)
            .or_else(|| config.get_api_key_for(ProviderKey::OpenAi))
            .ok_or_else(|| RociError::Authentication("Missing OPENAI_COMPAT_API_KEY".into()))?;
        let base_url = config
            .get_base_url_for(ProviderKey::OpenAiCompatible)
            .or_else(|| config.get_base_url_for(ProviderKey::OpenAi))
            .ok_or_else(|| RociError::Configuration("Missing OPENAI_COMPAT_BASE_URL".into()))?;
        Ok(Box::new(
            crate::provider::openai_compatible::OpenAiCompatibleProvider::new(
                model_id.to_string(),
                api_key,
                base_url,
            ),
        ))
    }
}

// ---------------------------------------------------------------------------
// GitHub Copilot
// ---------------------------------------------------------------------------

#[cfg(feature = "github-copilot")]
pub struct GitHubCopilotFactory;

#[cfg(feature = "github-copilot")]
fn github_copilot_static_catalog(provider_key: &str) -> ModelCatalog {
    crate::models::catalog::github_copilot_static_catalog(provider_key)
}

#[cfg(feature = "github-copilot")]
fn github_copilot_static_catalog_with_warning(provider_key: &str, warning: String) -> ModelCatalog {
    let mut catalog = github_copilot_static_catalog(provider_key);
    catalog.update_models(|model| {
        model.metadata.insert(
            "warning".to_string(),
            serde_json::Value::String(warning.clone()),
        );
    });
    catalog
}

#[cfg(feature = "github-copilot")]
fn resolve_github_copilot_credentials(config: &RociConfig) -> Result<(String, String), RociError> {
    // Try the copilot-api token first (saved by `roci auth login copilot`).
    // On load error, fall through to config-based fallback credentials.
    // Only hard-fail when the store error *and* no fallback creds exist.
    let (cached_key, cached_url, load_err) = if let Some(store) = config.token_store() {
        match store.load("github-copilot-api", "default") {
            Ok(Some(token)) => {
                let is_valid = token
                    .expires_at
                    .map(|exp| exp > chrono::Utc::now())
                    .unwrap_or(false);
                if is_valid {
                    let url = token.account_id.unwrap_or_default();
                    (
                        Some(token.access_token),
                        if url.is_empty() { None } else { Some(url) },
                        None,
                    )
                } else {
                    (None, None, None)
                }
            }
            Ok(None) => (None, None, None),
            Err(e) => (None, None, Some(e)),
        }
    } else {
        (None, None, None)
    };

    let api_key = cached_key
        .or_else(|| config.get_api_key_for(ProviderKey::GitHubCopilot))
        .ok_or_else(|| match load_err {
            Some(e) => RociError::Authentication(format!(
                "failed to load github-copilot-api credentials: {e}"
            )),
            None => RociError::MissingCredential {
                provider: "github-copilot".to_string(),
            },
        })?;
    let base_url = cached_url
        .or_else(|| config.get_base_url_for(ProviderKey::GitHubCopilot))
        .ok_or_else(|| RociError::MissingConfiguration {
            key: "base_url".to_string(),
            provider: "github-copilot".to_string(),
        })?;

    Ok((api_key, base_url))
}

#[cfg(feature = "github-copilot")]
fn should_fallback_to_copilot_static(error: &RociError) -> bool {
    match error {
        RociError::UnsupportedOperation(_) => true,
        RociError::Api { status, .. } => (500..=599).contains(status),
        RociError::Network(error) => error.is_timeout() || error.is_connect(),
        RociError::Timeout(_) => true,
        _ => false,
    }
}

#[cfg(feature = "github-copilot")]
fn fallback_warning(error: &RociError) -> Option<String> {
    match error {
        RociError::Api {
            status, message, ..
        } if (500..=599).contains(status) => Some(format!(
            "dynamic /models discovery failed with status {status}: {message}"
        )),
        RociError::Network(error) if error.is_timeout() || error.is_connect() => {
            Some(format!("dynamic /models discovery failed: {error}"))
        }
        RociError::Timeout(ms) => Some(format!("dynamic /models discovery timed out after {ms}ms")),
        _ => None,
    }
}

#[cfg(feature = "github-copilot")]
impl ProviderFactory for GitHubCopilotFactory {
    fn provider_keys(&self) -> &[&str] {
        &["github-copilot"]
    }

    fn list_models<'a>(
        &'a self,
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(async move {
            if !options.include_dynamic {
                return if options.include_static {
                    Ok(github_copilot_static_catalog(provider_key))
                } else {
                    Ok(ModelCatalog::default())
                };
            }

            let (api_key, base_url) = match resolve_github_copilot_credentials(config) {
                Ok(credentials) => credentials,
                Err(_error) if options.include_static => {
                    return Ok(github_copilot_static_catalog(provider_key));
                }
                Err(error) => return Err(error),
            };

            match crate::provider::github_copilot::list_copilot_models(
                &api_key,
                &base_url,
                provider_key,
            )
            .await
            {
                Ok(catalog) => Ok(catalog),
                Err(error)
                    if options.include_static && should_fallback_to_copilot_static(&error) =>
                {
                    if let Some(warning) = fallback_warning(&error) {
                        Ok(github_copilot_static_catalog_with_warning(
                            provider_key,
                            warning,
                        ))
                    } else {
                        Ok(github_copilot_static_catalog(provider_key))
                    }
                }
                Err(error) => Err(error),
            }
        })
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        let (api_key, base_url) = resolve_github_copilot_credentials(config)?;
        Ok(Box::new(
            crate::provider::github_copilot::GitHubCopilotProvider::new(
                model_id.to_string(),
                api_key,
                base_url,
            ),
        ))
    }
}

// ---------------------------------------------------------------------------
// Anthropic Compatible
// ---------------------------------------------------------------------------

#[cfg(feature = "anthropic-compatible")]
pub struct AnthropicCompatibleFactory;

#[cfg(feature = "anthropic-compatible")]
impl ProviderFactory for AnthropicCompatibleFactory {
    fn provider_keys(&self) -> &[&str] {
        &["anthropic-compatible"]
    }

    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        catalog_future(provider_key, options, crate::models::catalog::empty_catalog)
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        let api_key = config
            .get_api_key_for(ProviderKey::Anthropic)
            .ok_or_else(|| RociError::Authentication("Missing ANTHROPIC_COMPAT_API_KEY".into()))?;
        let base_url = config
            .get_base_url_for(ProviderKey::Anthropic)
            .ok_or_else(|| RociError::Configuration("Missing ANTHROPIC_COMPAT_BASE_URL".into()))?;
        Ok(Box::new(
            crate::provider::anthropic_compatible::AnthropicCompatibleProvider::new(
                model_id.to_string(),
                api_key,
                base_url,
            ),
        ))
    }
}

// ---------------------------------------------------------------------------
// Azure
// ---------------------------------------------------------------------------

#[cfg(feature = "azure")]
pub struct AzureFactory;

#[cfg(feature = "azure")]
impl ProviderFactory for AzureFactory {
    fn provider_keys(&self) -> &[&str] {
        &["azure"]
    }

    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        catalog_future(provider_key, options, crate::models::catalog::empty_catalog)
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        let api_key = config.get_api_key_for(ProviderKey::Azure).ok_or_else(|| {
            RociError::MissingCredential {
                provider: "azure".to_string(),
            }
        })?;
        let endpoint = config.get_base_url_for(ProviderKey::Azure).ok_or_else(|| {
            RociError::MissingConfiguration {
                key: "AZURE_OPENAI_ENDPOINT".to_string(),
                provider: "azure".to_string(),
            }
        })?;
        let api_version = "2024-06-01".to_string();
        Ok(Box::new(crate::provider::azure::AzureOpenAiProvider::new(
            endpoint,
            model_id.to_string(),
            api_key,
            api_version,
        )))
    }
}

// ---------------------------------------------------------------------------
// OpenRouter
// ---------------------------------------------------------------------------

#[cfg(feature = "openrouter")]
pub struct OpenRouterFactory;

#[cfg(feature = "openrouter")]
impl ProviderFactory for OpenRouterFactory {
    fn provider_keys(&self) -> &[&str] {
        &["openrouter"]
    }

    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        catalog_future(provider_key, options, crate::models::catalog::empty_catalog)
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        let api_key = optional_api_key(config, "openrouter");
        Ok(Box::new(
            crate::provider::openrouter::OpenRouterProvider::new(model_id.to_string(), api_key),
        ))
    }
}

// ---------------------------------------------------------------------------
// Together
// ---------------------------------------------------------------------------

#[cfg(feature = "together")]
pub struct TogetherFactory;

#[cfg(feature = "together")]
impl ProviderFactory for TogetherFactory {
    fn provider_keys(&self) -> &[&str] {
        &["together"]
    }

    fn list_models<'a>(
        &'a self,
        _config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        catalog_future(provider_key, options, crate::models::catalog::empty_catalog)
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        let api_key = optional_api_key(config, "together");
        Ok(Box::new(crate::provider::together::TogetherProvider::new(
            model_id.to_string(),
            api_key,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_without_credentials() -> RociConfig {
        RociConfig::new().with_token_store(None)
    }

    #[cfg(feature = "openai")]
    #[test]
    fn openai_factory_allows_missing_default_api_key() {
        let config = config_without_credentials();

        let provider = OpenAiFactory.create(&config, "openai", "gpt-4o");

        assert!(provider.is_ok());
    }

    #[cfg(feature = "openai")]
    #[test]
    fn codex_factory_allows_missing_default_api_key() {
        let config = config_without_credentials();

        let provider = CodexFactory.create(&config, "codex", "gpt-5-nano");

        assert!(provider.is_ok());
    }

    #[cfg(feature = "anthropic")]
    #[test]
    fn anthropic_factory_allows_missing_default_api_key() {
        let config = config_without_credentials();

        let provider = AnthropicFactory.create(&config, "anthropic", "claude-sonnet-4");

        assert!(provider.is_ok());
    }

    #[test]
    fn openrouter_api_key_ignores_openai_key() {
        let config = config_without_credentials();
        config.set_api_key("openai", "openai-key".to_string());

        assert_eq!(optional_api_key(&config, "openrouter"), "");
    }

    #[test]
    fn openrouter_api_key_reads_dedicated_key() {
        let config = config_without_credentials();
        config.set_api_key("openrouter", "openrouter-key".to_string());

        assert_eq!(optional_api_key(&config, "openrouter"), "openrouter-key");
    }

    #[test]
    fn together_api_key_ignores_openai_key() {
        let config = config_without_credentials();
        config.set_api_key("openai", "openai-key".to_string());

        assert_eq!(optional_api_key(&config, "together"), "");
    }

    #[test]
    fn together_api_key_reads_dedicated_key() {
        let config = config_without_credentials();
        config.set_api_key("together", "together-key".to_string());

        assert_eq!(optional_api_key(&config, "together"), "together-key");
    }

    #[cfg(feature = "openai")]
    #[tokio::test]
    async fn factory_registration_lists_static_openai_catalog() {
        let config = config_without_credentials();
        let mut registry = roci_core::provider::ProviderRegistry::new();
        crate::register_default_providers(&mut registry);
        let options = ModelListOptions {
            provider_key: Some("openai".to_string()),
            ..ModelListOptions::default()
        };

        let catalog = registry.list_models(&config, &options).await.unwrap();

        assert!(catalog
            .models()
            .iter()
            .any(|model| model.provider_key == "openai" && model.model_id == "gpt-4o"));
    }

    #[cfg(feature = "openai")]
    #[tokio::test]
    async fn static_factory_honors_include_static_false() {
        let config = config_without_credentials();
        let options = ModelListOptions {
            include_static: false,
            ..ModelListOptions::default()
        };

        let catalog = OpenAiFactory
            .list_models(&config, "openai", &options)
            .await
            .unwrap();

        assert!(catalog.models().is_empty());
    }

    #[cfg(feature = "github-copilot")]
    mod copilot {
        use super::*;
        use roci_core::models::ModelCatalogSource;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        fn config_with_copilot(base_url: String) -> RociConfig {
            let config = config_without_credentials();
            config.set_api_key("github-copilot", "test-token".to_string());
            config.set_base_url("github-copilot", base_url);
            config
        }

        fn static_model_count() -> usize {
            crate::models::catalog::github_copilot_static_catalog("github-copilot")
                .models()
                .len()
        }

        #[tokio::test]
        async fn missing_credentials_falls_back_to_static_when_enabled() {
            let config = config_without_credentials();

            let catalog = GitHubCopilotFactory
                .list_models(&config, "github-copilot", &ModelListOptions::default())
                .await
                .unwrap();

            assert_eq!(catalog.models().len(), static_model_count());
            assert!(matches!(
                catalog.models()[0].source,
                ModelCatalogSource::Static
            ));
        }

        #[tokio::test]
        async fn include_dynamic_false_skips_http_and_returns_static() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/models"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": [{"id": "dynamic-model"}]
                })))
                .expect(0)
                .mount(&server)
                .await;
            let config = config_with_copilot(server.uri());
            let options = ModelListOptions {
                include_dynamic: false,
                ..ModelListOptions::default()
            };

            let catalog = GitHubCopilotFactory
                .list_models(&config, "github-copilot", &options)
                .await
                .unwrap();

            assert_eq!(catalog.models().len(), static_model_count());
            assert!(catalog
                .models()
                .iter()
                .all(|model| matches!(model.source, ModelCatalogSource::Static)));
            server.verify().await;
        }

        #[tokio::test]
        async fn include_static_false_requires_credentials_and_configuration() {
            let options = ModelListOptions {
                include_static: false,
                ..ModelListOptions::default()
            };
            let missing_creds = config_without_credentials();

            let err = GitHubCopilotFactory
                .list_models(&missing_creds, "github-copilot", &options)
                .await
                .unwrap_err();

            assert!(matches!(err, RociError::MissingCredential { .. }));

            let missing_config = config_without_credentials();
            missing_config.set_api_key("github-copilot", "test-token".to_string());

            let err = GitHubCopilotFactory
                .list_models(&missing_config, "github-copilot", &options)
                .await
                .unwrap_err();

            assert!(matches!(err, RociError::MissingConfiguration { .. }));
        }

        #[tokio::test]
        async fn unsupported_models_endpoint_falls_back_to_static() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/models"))
                .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
                .mount(&server)
                .await;
            let config = config_with_copilot(server.uri());

            let catalog = GitHubCopilotFactory
                .list_models(&config, "github-copilot", &ModelListOptions::default())
                .await
                .unwrap();

            assert_eq!(catalog.models().len(), static_model_count());
            assert!(catalog
                .models()
                .iter()
                .all(|model| matches!(model.source, ModelCatalogSource::Static)));
        }

        #[tokio::test]
        async fn authentication_errors_do_not_fallback_to_static() {
            for status in [401, 403] {
                let server = MockServer::start().await;
                Mock::given(method("GET"))
                    .and(path("/models"))
                    .respond_with(ResponseTemplate::new(status).set_body_string("auth failed"))
                    .mount(&server)
                    .await;
                let config = config_with_copilot(server.uri());

                let err = GitHubCopilotFactory
                    .list_models(&config, "github-copilot", &ModelListOptions::default())
                    .await
                    .unwrap_err();

                assert!(matches!(err, RociError::Authentication(_)));
            }
        }

        #[tokio::test]
        async fn server_errors_fallback_to_static_with_warning_metadata() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/models"))
                .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
                .mount(&server)
                .await;
            let config = config_with_copilot(server.uri());

            let catalog = GitHubCopilotFactory
                .list_models(&config, "github-copilot", &ModelListOptions::default())
                .await
                .unwrap();

            assert_eq!(catalog.models().len(), static_model_count());
            assert!(catalog.models().iter().all(|model| {
                model
                    .metadata
                    .get("warning")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|warning| warning.contains("status 503"))
            }));
        }
    }
}
