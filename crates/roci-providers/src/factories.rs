//! ProviderFactory implementations for each built-in provider.

use futures::future::BoxFuture;
use roci_core::auth::{CredentialFlow, ProviderDescriptor};
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
    not(any(feature = "openrouter", feature = "together", test)),
    allow(dead_code)
)]
fn optional_api_key(config: &RociConfig, provider: &str) -> String {
    config.get_api_key(provider).unwrap_or_default()
}

fn explicit_descriptor(
    canonical_key: &'static str,
    display_name: &'static str,
    flows: &[CredentialFlow],
    endpoint_configurable: bool,
) -> ProviderDescriptor {
    ProviderDescriptor::new(
        canonical_key,
        display_name,
        flows.to_vec(),
        endpoint_configurable,
    )
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

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor("openai", "OpenAI", &[CredentialFlow::ApiKey], true)
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

        let (api_key, base_url) = config.get_api_key_and_base_url_for(ProviderKey::OpenAi);
        let api_key = api_key.unwrap_or_default();
        let model =
            OpenAiModel::from_str(model_id).unwrap_or(OpenAiModel::Custom(model_id.to_string()));
        if model.uses_responses_api() {
            Ok(Box::new(
                crate::provider::openai_responses::OpenAiResponsesProvider::new(
                    model, api_key, base_url, None,
                ),
            ))
        } else {
            Ok(Box::new(crate::provider::openai::OpenAiProvider::new(
                model, api_key, base_url, None,
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

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor("codex", "Codex", &[CredentialFlow::ApiKey], true)
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

        let (api_key, base_url) = config.get_api_key_and_base_url_for(ProviderKey::Codex);
        let api_key = api_key.unwrap_or_default();
        let base_url =
            base_url.or_else(|| Some("https://chatgpt.com/backend-api/codex".to_string()));
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

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor("anthropic", "Anthropic", &[CredentialFlow::ApiKey], true)
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

        let (api_key, base_url) = config.get_api_key_and_base_url_for(ProviderKey::Anthropic);
        let api_key = api_key.unwrap_or_default();
        let model = AnthropicModel::from_str(model_id)
            .unwrap_or(AnthropicModel::Custom(model_id.to_string()));
        Ok(Box::new(
            crate::provider::anthropic::AnthropicProvider::new(model, api_key, base_url),
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

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor("google", "Google", &[CredentialFlow::ApiKey], false)
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

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor("grok", "Grok", &[CredentialFlow::ApiKey], false)
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

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor("groq", "Groq", &[CredentialFlow::ApiKey], false)
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

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor("mistral", "Mistral", &[CredentialFlow::ApiKey], false)
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

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor("ollama", "Ollama", &[CredentialFlow::Local], true)
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

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor("lmstudio", "LM Studio", &[CredentialFlow::Local], true)
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

#[cfg(any(feature = "openai-compatible", feature = "anthropic-compatible"))]
fn resolve_dedicated_or_inherited_pair(
    dedicated: (Option<String>, Option<String>),
    inherited: (Option<String>, Option<String>),
    missing_key_message: &'static str,
    missing_url_message: &'static str,
) -> Result<(String, String), RociError> {
    let has_any_api_key = dedicated.0.is_some() || inherited.0.is_some();
    if let (Some(api_key), Some(base_url)) = dedicated {
        return Ok((api_key, base_url));
    }
    if let (Some(api_key), Some(base_url)) = inherited {
        return Ok((api_key, base_url));
    }
    if !has_any_api_key {
        Err(RociError::Authentication(missing_key_message.into()))
    } else {
        Err(RociError::Configuration(missing_url_message.into()))
    }
}

#[cfg(feature = "openai-compatible")]
fn resolve_openai_compatible_credentials(
    config: &RociConfig,
) -> Result<(String, String), RociError> {
    resolve_dedicated_or_inherited_pair(
        config.get_api_key_and_base_url_for(ProviderKey::OpenAiCompatible),
        config.get_api_key_and_base_url_for(ProviderKey::OpenAi),
        "Missing OPENAI_COMPAT_API_KEY",
        "Missing OPENAI_COMPAT_BASE_URL",
    )
}

#[cfg(feature = "openai-compatible")]
impl ProviderFactory for OpenAiCompatibleFactory {
    fn provider_keys(&self) -> &[&str] {
        &["openai-compatible"]
    }

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor(
            "openai-compatible",
            "OpenAI Compatible",
            &[CredentialFlow::ApiKey],
            true,
        )
    }

    fn is_available(&self, config: &RociConfig, _provider_key: &str) -> bool {
        resolve_openai_compatible_credentials(config).is_ok()
    }

    fn check_available(&self, config: &RociConfig, _provider_key: &str) -> Result<(), RociError> {
        resolve_openai_compatible_credentials(config).map(|_| ())
    }

    fn list_models<'a>(
        &'a self,
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(async move {
            if !options.include_unavailable {
                self.check_available(config, provider_key)?;
            }
            catalog_future(provider_key, options, crate::models::catalog::empty_catalog).await
        })
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        let (api_key, base_url) = resolve_openai_compatible_credentials(config)?;
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

    let cached = (cached_key, cached_url);
    let fallback = config.get_api_key_and_base_url_for(ProviderKey::GitHubCopilot);
    let has_any_api_key = cached.0.is_some() || fallback.0.is_some();
    if let (Some(api_key), Some(base_url)) = cached {
        return Ok((api_key, base_url));
    }
    if let (Some(api_key), Some(base_url)) = fallback {
        return Ok((api_key, base_url));
    }
    if !has_any_api_key {
        return Err(match load_err {
            Some(error) => RociError::Authentication(format!(
                "failed to load github-copilot-api credentials: {error}"
            )),
            None => RociError::MissingCredential {
                provider: "github-copilot".to_string(),
            },
        });
    }
    Err(RociError::MissingConfiguration {
        key: "base_url".to_string(),
        provider: "github-copilot".to_string(),
    })
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

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor("github-copilot", "GitHub Copilot", &[], false)
    }

    fn is_available(&self, config: &RociConfig, _provider_key: &str) -> bool {
        resolve_github_copilot_credentials(config).is_ok()
    }

    fn check_available(&self, config: &RociConfig, _provider_key: &str) -> Result<(), RociError> {
        resolve_github_copilot_credentials(config).map(|_| ())
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
fn resolve_anthropic_compatible_credentials(
    config: &RociConfig,
) -> Result<(String, String), RociError> {
    resolve_dedicated_or_inherited_pair(
        config.get_api_key_and_base_url("anthropic-compatible"),
        config.get_api_key_and_base_url_for(ProviderKey::Anthropic),
        "Missing ANTHROPIC_COMPAT_API_KEY",
        "Missing ANTHROPIC_COMPAT_BASE_URL",
    )
}

#[cfg(feature = "anthropic-compatible")]
impl ProviderFactory for AnthropicCompatibleFactory {
    fn provider_keys(&self) -> &[&str] {
        &["anthropic-compatible"]
    }

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor(
            "anthropic-compatible",
            "Anthropic Compatible",
            &[CredentialFlow::ApiKey],
            true,
        )
    }

    fn is_available(&self, config: &RociConfig, _provider_key: &str) -> bool {
        resolve_anthropic_compatible_credentials(config).is_ok()
    }

    fn check_available(&self, config: &RociConfig, _provider_key: &str) -> Result<(), RociError> {
        resolve_anthropic_compatible_credentials(config).map(|_| ())
    }

    fn list_models<'a>(
        &'a self,
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(async move {
            if !options.include_unavailable {
                self.check_available(config, provider_key)?;
            }
            catalog_future(provider_key, options, crate::models::catalog::empty_catalog).await
        })
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        let (api_key, base_url) = resolve_anthropic_compatible_credentials(config)?;
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
fn resolve_azure_credentials(config: &RociConfig) -> Result<(String, String), RociError> {
    let (api_key, endpoint) = config.get_api_key_and_base_url_for(ProviderKey::Azure);
    let api_key = api_key.ok_or_else(|| RociError::MissingCredential {
        provider: "azure".to_string(),
    })?;
    let endpoint = endpoint.ok_or_else(|| RociError::MissingConfiguration {
        key: "AZURE_OPENAI_ENDPOINT".to_string(),
        provider: "azure".to_string(),
    })?;
    Ok((api_key, endpoint))
}

#[cfg(feature = "azure")]
impl ProviderFactory for AzureFactory {
    fn provider_keys(&self) -> &[&str] {
        &["azure"]
    }

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor("azure", "Azure OpenAI", &[CredentialFlow::ApiKey], true)
    }

    fn is_available(&self, config: &RociConfig, _provider_key: &str) -> bool {
        resolve_azure_credentials(config).is_ok()
    }

    fn check_available(&self, config: &RociConfig, _provider_key: &str) -> Result<(), RociError> {
        resolve_azure_credentials(config).map(|_| ())
    }

    fn list_models<'a>(
        &'a self,
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(async move {
            if !options.include_unavailable {
                self.check_available(config, provider_key)?;
            }
            catalog_future(provider_key, options, crate::models::catalog::empty_catalog).await
        })
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        let (api_key, endpoint) = resolve_azure_credentials(config)?;
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

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor("openrouter", "OpenRouter", &[CredentialFlow::ApiKey], false)
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

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor("together", "Together", &[CredentialFlow::ApiKey], false)
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
        RociConfig::new()
            .with_token_store(None)
            .with_provider_credential_store(None)
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

    #[cfg(feature = "anthropic")]
    #[test]
    fn anthropic_registry_uses_stored_key_before_oauth_fallback() {
        use roci_core::auth::{
            FileTokenStore, InMemoryProviderCredentialStore, ProviderApiKey,
            ProviderCredentialRecord, ProviderCredentialStore, Token, TokenStore, TokenStoreConfig,
        };
        use std::sync::Arc;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let token_store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().to_path_buf(),
        )));
        token_store
            .save(
                "claude-code",
                "default",
                &Token {
                    access_token: "oauth-token".into(),
                    refresh_token: None,
                    id_token: None,
                    expires_at: None,
                    last_refresh: None,
                    scopes: None,
                    account_id: None,
                },
            )
            .unwrap();
        let credential_store = Arc::new(InMemoryProviderCredentialStore::default());
        credential_store
            .save(
                "anthropic",
                &ProviderCredentialRecord::new(ProviderApiKey::new("stored-key"), None),
            )
            .unwrap();
        let config = RociConfig::new()
            .with_token_store(Some(token_store))
            .with_provider_credential_store(Some(credential_store.clone()));
        let mut registry = roci_core::provider::ProviderRegistry::new();
        registry.register(Arc::new(AnthropicFactory));

        assert_eq!(config.get_api_key("anthropic"), Some("stored-key".into()));
        assert_eq!(registry.is_available("anthropic", &config), Some(true));
        assert!(registry
            .create_provider("anthropic", "claude-sonnet-4", &config)
            .is_ok());

        credential_store.clear("anthropic").unwrap();
        assert_eq!(config.get_api_key("anthropic"), Some("oauth-token".into()));
        assert_eq!(registry.is_available("anthropic", &config), Some(true));
        assert!(registry
            .create_provider("anthropic", "claude-sonnet-4", &config)
            .is_ok());
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
            include_unavailable: true,
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

        #[test]
        fn is_available_requires_api_token_and_base_url() {
            let missing = config_without_credentials();
            assert!(!GitHubCopilotFactory.is_available(&missing, "github-copilot"));

            let key_only = config_without_credentials();
            key_only.set_api_key("github-copilot", "token".to_string());
            assert!(!GitHubCopilotFactory.is_available(&key_only, "github-copilot"));

            let ready = config_with_copilot("https://api.example".to_string());
            assert!(GitHubCopilotFactory.is_available(&ready, "github-copilot"));
        }

        #[test]
        fn is_available_accepts_github_copilot_api_token_store() {
            use chrono::{Duration, Utc};
            use roci_core::auth::store::{FileTokenStore, TokenStore, TokenStoreConfig};
            use roci_core::auth::token::Token;
            use std::sync::Arc;
            use tempfile::TempDir;

            let dir = TempDir::new().unwrap();
            let store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
                dir.path().to_path_buf(),
            )));
            store
                .save(
                    "github-copilot-api",
                    "default",
                    &Token {
                        access_token: "api-token".to_string(),
                        refresh_token: None,
                        id_token: None,
                        expires_at: Some(Utc::now() + Duration::hours(1)),
                        last_refresh: None,
                        scopes: None,
                        account_id: Some("https://api.githubcopilot.com".to_string()),
                    },
                )
                .unwrap();

            let config = RociConfig::new()
                .with_token_store(Some(store))
                .with_provider_credential_store(None);

            // github-copilot-api is a distinct store key from provider-key credentials.
            assert!(!config.has_credentials("github-copilot"));
            assert!(GitHubCopilotFactory.is_available(&config, "github-copilot"));
        }
    }

    #[cfg(feature = "openai-compatible")]
    #[test]
    fn openai_compatible_is_available_needs_key_and_endpoint_aliases() {
        let missing = config_without_credentials();
        assert!(!OpenAiCompatibleFactory.is_available(&missing, "openai-compatible"));

        let key_only = config_without_credentials();
        key_only.set_api_key("openai-compatible", "compat-key".to_string());
        assert!(!OpenAiCompatibleFactory.is_available(&key_only, "openai-compatible"));

        let dedicated = config_without_credentials();
        dedicated.set_api_key("openai-compatible", "compat-key".to_string());
        dedicated.set_base_url("openai-compatible", "https://compat.example".to_string());
        assert!(OpenAiCompatibleFactory.is_available(&dedicated, "openai-compatible"));

        let via_openai = config_without_credentials();
        via_openai.set_api_key("openai", "openai-key".to_string());
        via_openai.set_base_url("openai", "https://api.openai.com/v1".to_string());
        assert!(OpenAiCompatibleFactory.is_available(&via_openai, "openai-compatible"));

        let partial_dedicated = config_without_credentials();
        partial_dedicated.set_api_key("openai-compatible", "partial-key".to_string());
        partial_dedicated.set_api_key("openai", "openai-key".to_string());
        partial_dedicated.set_base_url("openai", "https://api.openai.com/v1".to_string());
        assert_eq!(
            resolve_openai_compatible_credentials(&partial_dedicated).unwrap(),
            (
                "openai-key".to_string(),
                "https://api.openai.com/v1".to_string()
            )
        );
    }

    #[cfg(feature = "anthropic-compatible")]
    #[test]
    fn anthropic_compatible_is_available_with_dedicated_or_inherited_config() {
        let missing = config_without_credentials();
        assert!(!AnthropicCompatibleFactory.is_available(&missing, "anthropic-compatible"));

        let dedicated = config_without_credentials();
        dedicated.set_api_key("anthropic-compatible", "compat-key".to_string());
        dedicated.set_base_url("anthropic-compatible", "https://compat.example".to_string());
        assert!(AnthropicCompatibleFactory.is_available(&dedicated, "anthropic-compatible"));

        let inherited = config_without_credentials();
        inherited.set_api_key("anthropic", "anthropic-key".to_string());
        inherited.set_base_url("anthropic", "https://api.anthropic.com".to_string());
        assert!(AnthropicCompatibleFactory.is_available(&inherited, "anthropic-compatible"));

        let partial_dedicated = config_without_credentials();
        partial_dedicated.set_api_key("anthropic-compatible", "partial-key".to_string());
        partial_dedicated.set_api_key("anthropic", "anthropic-key".to_string());
        partial_dedicated.set_base_url("anthropic", "https://api.anthropic.com".to_string());
        assert_eq!(
            resolve_anthropic_compatible_credentials(&partial_dedicated).unwrap(),
            (
                "anthropic-key".to_string(),
                "https://api.anthropic.com".to_string()
            )
        );
    }

    #[cfg(feature = "ollama")]
    #[test]
    fn local_ollama_is_available_without_credentials() {
        let config = config_without_credentials();
        assert!(OllamaFactory.is_available(&config, "ollama"));
    }

    #[cfg(feature = "lmstudio")]
    #[test]
    fn local_lmstudio_is_available_without_credentials() {
        let config = config_without_credentials();
        assert!(LmStudioFactory.is_available(&config, "lmstudio"));
    }

    #[cfg(feature = "azure")]
    #[test]
    fn azure_is_available_needs_key_and_endpoint() {
        let missing = config_without_credentials();
        assert!(!AzureFactory.is_available(&missing, "azure"));

        let key_only = config_without_credentials();
        key_only.set_api_key("azure", "azure-key".to_string());
        assert!(!AzureFactory.is_available(&key_only, "azure"));

        let ready = config_without_credentials();
        ready.set_api_key("azure", "azure-key".to_string());
        ready.set_base_url("azure", "https://example.openai.azure.com".to_string());
        assert!(AzureFactory.is_available(&ready, "azure"));
    }

    #[cfg(feature = "google")]
    #[test]
    fn unavailable_remote_is_not_available_without_credentials() {
        let config = config_without_credentials();
        assert!(!GoogleFactory.is_available(&config, "google"));

        config.set_api_key("google", "google-key".to_string());
        assert!(GoogleFactory.is_available(&config, "google"));
    }

    #[test]
    fn built_in_descriptors_match_explicit_table() {
        // Drift guard: every built-in factory must keep an explicit descriptor.
        let expected: &[(&str, &str, &[CredentialFlow], bool)] = &[
            #[cfg(feature = "openai")]
            ("openai", "OpenAI", &[CredentialFlow::ApiKey], true),
            #[cfg(feature = "openai")]
            ("codex", "Codex", &[CredentialFlow::ApiKey], true),
            #[cfg(feature = "anthropic")]
            ("anthropic", "Anthropic", &[CredentialFlow::ApiKey], true),
            #[cfg(feature = "google")]
            ("google", "Google", &[CredentialFlow::ApiKey], false),
            #[cfg(feature = "grok")]
            ("grok", "Grok", &[CredentialFlow::ApiKey], false),
            #[cfg(feature = "groq")]
            ("groq", "Groq", &[CredentialFlow::ApiKey], false),
            #[cfg(feature = "mistral")]
            ("mistral", "Mistral", &[CredentialFlow::ApiKey], false),
            #[cfg(feature = "ollama")]
            ("ollama", "Ollama", &[CredentialFlow::Local], true),
            #[cfg(feature = "lmstudio")]
            ("lmstudio", "LM Studio", &[CredentialFlow::Local], true),
            #[cfg(feature = "openai-compatible")]
            (
                "openai-compatible",
                "OpenAI Compatible",
                &[CredentialFlow::ApiKey],
                true,
            ),
            #[cfg(feature = "github-copilot")]
            ("github-copilot", "GitHub Copilot", &[], false),
            #[cfg(feature = "anthropic-compatible")]
            (
                "anthropic-compatible",
                "Anthropic Compatible",
                &[CredentialFlow::ApiKey],
                true,
            ),
            #[cfg(feature = "azure")]
            ("azure", "Azure OpenAI", &[CredentialFlow::ApiKey], true),
            #[cfg(feature = "openrouter")]
            ("openrouter", "OpenRouter", &[CredentialFlow::ApiKey], false),
            #[cfg(feature = "together")]
            ("together", "Together", &[CredentialFlow::ApiKey], false),
        ];

        let mut registry = roci_core::provider::ProviderRegistry::new();
        crate::register_default_providers(&mut registry);

        let mut seen = std::collections::BTreeSet::new();
        for key in registry.provider_keys() {
            let factory = registry.factory(key).expect("factory");
            let descriptor = factory.descriptor();
            assert_eq!(
                descriptor.canonical_key, key,
                "descriptor canonical key must match registered key for {key}"
            );
            assert!(
                factory
                    .provider_keys()
                    .contains(&descriptor.canonical_key.as_str()),
                "canonical key must be one of provider_keys for {key}"
            );
            seen.insert(key.to_string());

            let exp = expected
                .iter()
                .find(|(k, ..)| *k == key)
                .unwrap_or_else(|| panic!("missing expected descriptor row for {key}"));
            assert_eq!(descriptor.display_name, exp.1, "display drift for {key}");
            assert_eq!(
                descriptor.credential_flows.as_slice(),
                exp.2,
                "flow drift for {key}"
            );
            assert_eq!(
                descriptor.endpoint_configurable, exp.3,
                "endpoint drift for {key}"
            );
        }

        for (key, ..) in expected {
            assert!(
                seen.contains(*key),
                "expected built-in {key} missing from registry"
            );
        }
    }

    #[cfg(all(feature = "anthropic", feature = "openai", feature = "github-copilot"))]
    #[test]
    fn auth_manager_overlays_oauth_flows_and_rejects_unknown() {
        use roci_core::auth::{
            AuthService, ConfiguredSource, ProviderAuthManager, ProviderAuthState,
        };
        use std::sync::Arc;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let store = Arc::new(roci_core::auth::FileTokenStore::new(
            roci_core::auth::TokenStoreConfig::new(dir.path().to_path_buf()),
        ));
        let mut registry = roci_core::provider::ProviderRegistry::new();
        crate::register_default_providers(&mut registry);
        let mut auth = AuthService::new(store.clone());
        crate::register_default_auth_backends(&mut auth);
        let config = RociConfig::new()
            .with_token_store(Some(store))
            .with_provider_credential_store(Some(Arc::new(
                roci_core::auth::InMemoryProviderCredentialStore::default(),
            )));
        let manager = ProviderAuthManager::new(auth, registry, config).unwrap();

        let anthropic = manager.descriptor("anthropic").unwrap();
        assert_eq!(
            anthropic.credential_flows,
            vec![CredentialFlow::ApiKey, CredentialFlow::Pkce]
        );
        let codex = manager.descriptor("codex").unwrap();
        assert!(codex.credential_flows.contains(&CredentialFlow::DeviceCode));
        let copilot = manager.descriptor("github-copilot").unwrap();
        assert_eq!(copilot.credential_flows, vec![CredentialFlow::DeviceCode]);

        let unknown = manager.status("nope").unwrap_err();
        assert!(matches!(
            unknown,
            roci_core::auth::AuthError::UnknownProvider(_)
        ));

        let unconfigured = manager.status("anthropic").unwrap();
        assert_eq!(unconfigured.auth_state, ProviderAuthState::SignedOut);
        assert!(unconfigured.configured_sources.is_empty());
        assert!(!unconfigured.launch_available);

        manager.config().set_api_key("anthropic", "sk-test".into());
        let configured = manager.status("anthropic").unwrap();
        assert_eq!(
            configured.configured_sources,
            vec![ConfiguredSource::ExternallyConfigured]
        );
        assert!(configured.launch_available);
        let json = serde_json::to_string(&configured).unwrap();
        assert!(!json.contains("sk-test"));
    }
}
