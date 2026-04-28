//! ProviderFactory implementations for each built-in provider.

use roci_core::config::RociConfig;
use roci_core::error::RociError;
use roci_core::models::ProviderKey;
use roci_core::provider::{ModelProvider, ProviderFactory};

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
impl ProviderFactory for GitHubCopilotFactory {
    fn provider_keys(&self) -> &[&str] {
        &["github-copilot"]
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
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
}
