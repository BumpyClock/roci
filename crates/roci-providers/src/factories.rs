//! ProviderFactory implementations for each built-in provider.

#[cfg(any(feature = "openai", feature = "anthropic", feature = "google"))]
use futures::future::BoxFuture;
#[cfg(any(test, feature = "openai", feature = "anthropic", feature = "google"))]
use roci_core::auth::CredentialFlow;
#[cfg(any(feature = "openai", feature = "anthropic", feature = "google"))]
use roci_core::auth::ProviderDescriptor;
#[cfg(any(test, feature = "openai", feature = "anthropic", feature = "google"))]
use roci_core::auth::{CredentialMaterial, ResolvedProviderCredential};
#[cfg(any(test, feature = "openai", feature = "anthropic", feature = "google"))]
use roci_core::config::RociConfig;
#[cfg(any(test, feature = "openai", feature = "anthropic", feature = "google"))]
use roci_core::error::RociError;
#[cfg(any(feature = "openai", feature = "anthropic", feature = "google"))]
use roci_core::models::{ModelCatalog, ModelListOptions};
#[cfg(any(feature = "openai", feature = "anthropic", feature = "google"))]
use roci_core::provider::{ModelProvider, ProviderFactory};

/// Adapt a selected API-key credential to a transport without losing its endpoint.
/// Native OAuth must be handled before entering an API-key transport.
#[cfg(any(test, feature = "openai", feature = "anthropic", feature = "google"))]
fn api_key_pair(
    config: &RociConfig,
    provider: &str,
    credential: Option<ResolvedProviderCredential>,
) -> Result<(Option<String>, Option<String>), RociError> {
    match credential {
        Some(ResolvedProviderCredential {
            material: CredentialMaterial::ApiKey(key),
            endpoint,
        }) => Ok((
            Some(key.expose_secret().to_owned()),
            endpoint.map(|url| url.as_str().to_owned()),
        )),
        Some(_) => Err(RociError::UnsupportedOperation(format!(
            "{provider} requires an API key for this transport"
        ))),
        None => Ok((
            None,
            config
                .resolve_provider_endpoint(provider)?
                .map(|url| url.as_str().to_owned()),
        )),
    }
}

#[cfg(any(feature = "openai", feature = "anthropic", feature = "google"))]
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
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(crate::models::remote::list_configured_models(
            config,
            provider_key,
            options,
            "https://api.openai.com/v1",
            |id| {
                use crate::models::openai::OpenAiModel;
                id.parse::<OpenAiModel>()
                    .unwrap_or_else(|_| OpenAiModel::Custom(id.into()))
                    .capabilities()
            },
        ))
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::openai::OpenAiModel;
        use std::str::FromStr;

        let (api_key, base_url) = api_key_pair(
            config,
            "openai",
            config.resolve_provider_credential("openai")?,
        )?;
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
        config: &'a RociConfig,
        _provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(crate::auth::factory::codex_models(config, options))
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::openai::OpenAiModel;
        use std::str::FromStr;

        let credential = config.resolve_provider_credential("codex")?;
        if let Some(provider) =
            crate::auth::factory::managed_provider(config, "codex", model_id, credential.as_ref())?
        {
            return Ok(provider);
        }
        let (api_key, base_url) = api_key_pair(config, "codex", credential)?;
        let api_key = api_key.unwrap_or_default();
        let base_url =
            base_url.or_else(|| Some("https://chatgpt.com/backend-api/codex".to_string()));
        let account_id = config.get_account_id("codex");
        let model =
            OpenAiModel::from_str(model_id).unwrap_or(OpenAiModel::Custom(model_id.to_string()));
        Ok(Box::new(
            crate::provider::openai_responses::OpenAiResponsesProvider::new(
                model, api_key, base_url, account_id,
            ),
        ))
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
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(crate::models::anthropic_catalog::list_models(
            config,
            provider_key,
            options,
        ))
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::anthropic::AnthropicModel;
        use std::str::FromStr;

        let credential = config.resolve_provider_credential("anthropic")?;
        if let Some(provider) = crate::auth::factory::managed_provider(
            config,
            "anthropic",
            model_id,
            credential.as_ref(),
        )? {
            return Ok(provider);
        }
        let (api_key, base_url) = api_key_pair(config, "anthropic", credential)?;
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
        explicit_descriptor("google", "Google", &[CredentialFlow::ApiKey], true)
    }

    fn list_models<'a>(
        &'a self,
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(crate::models::google_catalog::list_models(
            config,
            provider_key,
            options,
        ))
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::google::GoogleModel;
        use std::str::FromStr;

        let credential = config.resolve_provider_credential("google")?;
        if let Some(provider) =
            crate::auth::factory::managed_provider(config, "google", model_id, credential.as_ref())?
        {
            return Ok(provider);
        }
        let (api_key, base_url) = api_key_pair(config, "google", credential)?;
        let api_key = api_key.ok_or_else(|| RociError::MissingCredential {
            provider: "google".into(),
        })?;
        let model =
            GoogleModel::from_str(model_id).unwrap_or(GoogleModel::Custom(model_id.to_string()));
        Ok(Box::new(crate::provider::google::GoogleProvider::new(
            model, api_key, base_url,
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
        explicit_descriptor("grok", "Grok", &[CredentialFlow::ApiKey], true)
    }

    fn list_models<'a>(
        &'a self,
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(crate::models::remote::list_configured_models(
            config,
            provider_key,
            options,
            "https://api.x.ai/v1",
            |id| {
                use crate::models::grok::GrokModel;
                id.parse::<GrokModel>()
                    .unwrap_or_else(|_| GrokModel::Custom(id.into()))
                    .capabilities()
            },
        ))
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::grok::GrokModel;
        use std::str::FromStr;

        let credential = config.resolve_provider_credential("grok")?;
        if let Some(provider) =
            crate::auth::factory::managed_provider(config, "grok", model_id, credential.as_ref())?
        {
            return Ok(provider);
        }
        let (api_key, base_url) = api_key_pair(config, "grok", credential)?;
        let api_key = api_key.ok_or_else(|| RociError::MissingCredential {
            provider: "grok".into(),
        })?;
        let model =
            GrokModel::from_str(model_id).unwrap_or(GrokModel::Custom(model_id.to_string()));
        Ok(Box::new(crate::provider::grok::GrokProvider::new(
            model, api_key, base_url,
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
        explicit_descriptor("groq", "Groq", &[CredentialFlow::ApiKey], true)
    }

    fn list_models<'a>(
        &'a self,
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(crate::models::remote::list_configured_models(
            config,
            provider_key,
            options,
            "https://api.groq.com/openai/v1",
            |id| {
                use crate::models::groq::GroqModel;
                id.parse::<GroqModel>()
                    .unwrap_or_else(|_| GroqModel::Custom(id.into()))
                    .capabilities()
            },
        ))
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::groq::GroqModel;
        use std::str::FromStr;

        let (api_key, base_url) =
            api_key_pair(config, "groq", config.resolve_provider_credential("groq")?)?;
        let api_key = api_key.ok_or_else(|| RociError::MissingCredential {
            provider: "groq".into(),
        })?;
        let model =
            GroqModel::from_str(model_id).unwrap_or(GroqModel::Custom(model_id.to_string()));
        Ok(Box::new(crate::provider::groq::GroqProvider::new(
            model, api_key, base_url,
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
        explicit_descriptor("mistral", "Mistral", &[CredentialFlow::ApiKey], true)
    }

    fn list_models<'a>(
        &'a self,
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(crate::models::remote::list_configured_models(
            config,
            provider_key,
            options,
            "https://api.mistral.ai/v1",
            |id| {
                use crate::models::mistral::MistralModel;
                id.parse::<MistralModel>()
                    .unwrap_or_else(|_| MistralModel::Custom(id.into()))
                    .capabilities()
            },
        ))
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::mistral::MistralModel;
        use std::str::FromStr;

        let (api_key, base_url) = api_key_pair(
            config,
            "mistral",
            config.resolve_provider_credential("mistral")?,
        )?;
        let api_key = api_key.ok_or_else(|| RociError::MissingCredential {
            provider: "mistral".into(),
        })?;
        let model =
            MistralModel::from_str(model_id).unwrap_or(MistralModel::Custom(model_id.to_string()));
        Ok(Box::new(crate::provider::mistral::MistralProvider::new(
            model, api_key, base_url,
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
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(async move {
            if !options.include_dynamic {
                return Ok(ModelCatalog::new());
            }
            let base = config
                .resolve_provider_endpoint("ollama")?
                .map(|url| url.as_str().to_owned())
                .unwrap_or_else(|| "http://localhost:11434".into());
            crate::models::remote::fetch_openai_models(
                provider_key,
                &format!("{}/v1/models", base.trim_end_matches('/')),
                None,
                |id| crate::models::ollama::OllamaModel::Custom(id.into()).capabilities(),
                true,
            )
            .await
        })
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
            .resolve_provider_endpoint("ollama")?
            .map(|url| url.as_str().to_owned())
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
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(async move {
            if !options.include_dynamic {
                return Ok(ModelCatalog::new());
            }
            let base = config
                .resolve_provider_endpoint("lmstudio")?
                .map(|url| url.as_str().to_owned())
                .unwrap_or_else(|| "http://localhost:1234".into());
            crate::models::remote::fetch_openai_models(
                provider_key,
                &format!("{}/v1/models", base.trim_end_matches('/')),
                None,
                |id| crate::models::lmstudio::LmStudioModel::Custom(id.into()).capabilities(),
                true,
            )
            .await
        })
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        use crate::models::lmstudio::LmStudioModel;

        let base_url = config
            .resolve_provider_endpoint("lmstudio")?
            .map(|url| url.as_str().to_owned())
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
fn resolve_compatible_credentials(
    config: &RociConfig,
    dedicated: &str,
    inherited: &str,
    missing_key_message: &'static str,
    missing_url_message: &'static str,
) -> Result<(String, String), RociError> {
    let first = api_key_pair(
        config,
        dedicated,
        config.resolve_provider_credential(dedicated)?,
    )?;
    let mut has_key = first.0.is_some();
    if let (Some(key), Some(endpoint)) = first {
        return Ok((key, endpoint));
    }
    let second = api_key_pair(
        config,
        inherited,
        config.resolve_provider_credential(inherited)?,
    )?;
    has_key |= second.0.is_some();
    if let (Some(key), Some(endpoint)) = second {
        return Ok((key, endpoint));
    }
    if has_key {
        Err(RociError::Configuration(missing_url_message.into()))
    } else {
        Err(RociError::Authentication(missing_key_message.into()))
    }
}

#[cfg(feature = "openai-compatible")]
fn resolve_openai_compatible_credentials(
    config: &RociConfig,
) -> Result<(String, String), RociError> {
    resolve_compatible_credentials(
        config,
        "openai-compatible",
        "openai",
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
            if !options.include_dynamic {
                return Ok(ModelCatalog::default());
            }
            let (key, base_url) = resolve_openai_compatible_credentials(config)?;
            let endpoint = crate::models::remote::models_endpoint(&base_url)?;
            crate::models::remote::fetch_openai_models(
                provider_key,
                &endpoint,
                Some(&key),
                |id| crate::models::openai::OpenAiModel::Custom(id.into()).capabilities(),
                false,
            )
            .await
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
fn resolve_github_copilot_credentials(
    config: &RociConfig,
    credential: Option<&ResolvedProviderCredential>,
) -> Result<(String, String), RociError> {
    if let Some(ResolvedProviderCredential {
        material: CredentialMaterial::ApiKey(key),
        endpoint,
    }) = credential
    {
        return Ok((
            key.expose_secret().to_owned(),
            endpoint
                .as_ref()
                .ok_or_else(|| RociError::MissingConfiguration {
                    key: "base_url".into(),
                    provider: "github-copilot".into(),
                })?
                .as_str()
                .to_owned(),
        ));
    }
    // A still-valid derived credential remains usable without a primary login.
    // Never treat the primary GitHub OAuth token as a Copilot API credential.
    let cached = config
        .token_store()
        .map(|store| store.load("github-copilot-api", "default"))
        .transpose()?
        .flatten()
        .filter(|token| {
            token
                .expires_at
                .is_some_and(|expiry| expiry > chrono::Utc::now())
        });
    let token = cached.ok_or_else(|| RociError::MissingCredential {
        provider: "github-copilot".into(),
    })?;
    let endpoint = credential
        .and_then(|credential| credential.endpoint.as_ref())
        .map(|url| url.as_str().to_owned())
        .or(token.account_id)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| RociError::MissingConfiguration {
            key: "base_url".into(),
            provider: "github-copilot".into(),
        })?;
    Ok((token.access_token, endpoint))
}

#[cfg(feature = "github-copilot")]
impl ProviderFactory for GitHubCopilotFactory {
    fn provider_keys(&self) -> &[&str] {
        &["github-copilot"]
    }

    fn descriptor(&self) -> ProviderDescriptor {
        explicit_descriptor("github-copilot", "GitHub Copilot", &[], false)
    }

    fn is_available(&self, config: &RociConfig, provider_key: &str) -> bool {
        self.check_available(config, provider_key).is_ok()
    }

    fn check_available(&self, config: &RociConfig, _provider_key: &str) -> Result<(), RociError> {
        let credential = config.resolve_provider_credential("github-copilot")?;
        if matches!(credential.as_ref().map(|credential| &credential.material), Some(CredentialMaterial::OAuth(token)) if token.is_valid() || token.refresh_token.is_some())
        {
            return Ok(());
        }
        resolve_github_copilot_credentials(config, credential.as_ref()).map(|_| ())
    }

    fn list_models<'a>(
        &'a self,
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(async move {
            if !options.include_dynamic {
                return Ok(ModelCatalog::new());
            }
            let credential = config.resolve_provider_credential("github-copilot")?;
            let (api_key, base_url) =
                match crate::auth::factory::copilot_credentials(config, None, credential.as_ref())
                    .await?
                {
                    Some(credentials) => credentials,
                    None => resolve_github_copilot_credentials(config, credential.as_ref())?,
                };
            let result = crate::provider::github_copilot::list_copilot_models(
                &api_key,
                &base_url,
                provider_key,
            )
            .await;
            if matches!(result, Err(RociError::Api { status: 401, .. })) {
                if let Some((api_key, base_url)) = crate::auth::factory::copilot_credentials(
                    config,
                    Some(&api_key),
                    credential.as_ref(),
                )
                .await?
                {
                    return crate::provider::github_copilot::list_copilot_models(
                        &api_key,
                        &base_url,
                        provider_key,
                    )
                    .await;
                }
            }
            result
        })
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        let credential = config.resolve_provider_credential("github-copilot")?;
        if let Some(provider) = crate::auth::factory::managed_provider(
            config,
            "github-copilot",
            model_id,
            credential.as_ref(),
        )? {
            return Ok(provider);
        }
        let (api_key, base_url) = resolve_github_copilot_credentials(config, credential.as_ref())?;
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
    resolve_compatible_credentials(
        config,
        "anthropic-compatible",
        "anthropic",
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
            if !options.include_dynamic {
                return Ok(ModelCatalog::default());
            }
            let (key, endpoint) = resolve_anthropic_compatible_credentials(config)?;
            crate::models::anthropic_catalog::fetch(provider_key, &endpoint, &key, false).await
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
    let (api_key, endpoint) = api_key_pair(
        config,
        "azure",
        config.resolve_provider_credential("azure")?,
    )?;
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
        _provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(async move {
            if !options.include_dynamic {
                return Ok(ModelCatalog::default());
            }
            match resolve_azure_credentials(config) {
                Ok(_) => Err(RociError::ModelDiscoveryUnsupported {
                    provider: "azure".into(),
                    reason: "Azure model selection requires deployment names; deployment discovery requires Azure management credentials, which this provider does not configure".into(),
                }),
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
        explicit_descriptor("openrouter", "OpenRouter", &[CredentialFlow::ApiKey], true)
    }

    fn list_models<'a>(
        &'a self,
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(crate::models::remote::list_configured_models(
            config,
            provider_key,
            options,
            "https://openrouter.ai/api/v1",
            |id| {
                use crate::models::openai::OpenAiModel;
                id.parse::<OpenAiModel>()
                    .unwrap_or_else(|_| OpenAiModel::Custom(id.into()))
                    .capabilities()
            },
        ))
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        let (api_key, base_url) = api_key_pair(
            config,
            "openrouter",
            config.resolve_provider_credential("openrouter")?,
        )?;
        let api_key = api_key.unwrap_or_default();
        Ok(Box::new(
            crate::provider::openrouter::OpenRouterProvider::new(
                model_id.to_string(),
                api_key,
                base_url,
            ),
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
        explicit_descriptor("together", "Together", &[CredentialFlow::ApiKey], true)
    }

    fn list_models<'a>(
        &'a self,
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(crate::models::remote::list_configured_models(
            config,
            provider_key,
            options,
            "https://api.together.xyz/v1",
            |id| {
                use crate::models::openai::OpenAiModel;
                id.parse::<OpenAiModel>()
                    .unwrap_or_else(|_| OpenAiModel::Custom(id.into()))
                    .capabilities()
            },
        ))
    }

    fn create(
        &self,
        config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        let (api_key, base_url) = api_key_pair(
            config,
            "together",
            config.resolve_provider_credential("together")?,
        )?;
        let api_key = api_key.unwrap_or_default();
        Ok(Box::new(crate::provider::together::TogetherProvider::new(
            model_id.to_string(),
            api_key,
            base_url,
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
                    provider_metadata: None,
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

        assert_eq!(
            config
                .resolve_provider_credential("anthropic")
                .unwrap()
                .map(|credential| match credential.material {
                    CredentialMaterial::ApiKey(key) => key.expose_secret().to_owned(),
                    CredentialMaterial::OAuth(token) => token.access_token,
                }),
            Some("stored-key".into())
        );
        assert_eq!(registry.is_available("anthropic", &config), Some(true));
        assert!(registry
            .create_provider("anthropic", "claude-sonnet-4", &config)
            .is_ok());

        credential_store.clear("anthropic").unwrap();
        assert_eq!(
            config
                .resolve_provider_credential("anthropic")
                .unwrap()
                .map(|credential| match credential.material {
                    CredentialMaterial::ApiKey(key) => key.expose_secret().to_owned(),
                    CredentialMaterial::OAuth(token) => token.access_token,
                }),
            Some("oauth-token".into())
        );
        assert_eq!(registry.is_available("anthropic", &config), Some(true));
        assert!(registry
            .create_provider("anthropic", "claude-sonnet-4", &config)
            .is_ok());
    }

    #[test]
    fn openrouter_api_key_ignores_openai_key() {
        let config = config_without_credentials();
        config.set_api_key("openai", "openai-key".to_string());

        assert_eq!(
            api_key_pair(
                &config,
                "openrouter",
                config.resolve_provider_credential("openrouter").unwrap()
            )
            .unwrap()
            .0
            .unwrap_or_default(),
            ""
        );
    }

    #[test]
    fn openrouter_api_key_reads_dedicated_key() {
        let config = config_without_credentials();
        config.set_api_key("openrouter", "openrouter-key".to_string());

        assert_eq!(
            api_key_pair(
                &config,
                "openrouter",
                config.resolve_provider_credential("openrouter").unwrap()
            )
            .unwrap()
            .0
            .unwrap_or_default(),
            "openrouter-key"
        );
    }

    #[test]
    fn together_api_key_ignores_openai_key() {
        let config = config_without_credentials();
        config.set_api_key("openai", "openai-key".to_string());

        assert_eq!(
            api_key_pair(
                &config,
                "together",
                config.resolve_provider_credential("together").unwrap()
            )
            .unwrap()
            .0
            .unwrap_or_default(),
            ""
        );
    }

    #[test]
    fn together_api_key_reads_dedicated_key() {
        let config = config_without_credentials();
        config.set_api_key("together", "together-key".to_string());

        assert_eq!(
            api_key_pair(
                &config,
                "together",
                config.resolve_provider_credential("together").unwrap()
            )
            .unwrap()
            .0
            .unwrap_or_default(),
            "together-key"
        );
    }

    #[cfg(feature = "openai")]
    #[tokio::test]
    async fn factory_registration_lists_live_openai_catalog() {
        use wiremock::{
            matchers::{header, method, path},
            Mock, MockServer, ResponseTemplate,
        };
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data":[{"id":"future-openai-model"}]})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let config = config_without_credentials();
        config.set_api_key("openai", "test-key".into());
        config.set_base_url("openai", format!("{}/v1", server.uri()));
        let mut registry = roci_core::provider::ProviderRegistry::new();
        crate::register_default_providers(&mut registry);
        let options = ModelListOptions {
            provider_key: Some("openai".into()),
            include_static: false,
            ..Default::default()
        };
        let catalog = registry.list_models(&config, &options).await.unwrap();
        assert_eq!(catalog.models().len(), 1);
        assert_eq!(catalog.models()[0].model_id, "future-openai-model");
        assert!(matches!(
            catalog.models()[0].source,
            roci_core::models::ModelCatalogSource::Dynamic { .. }
        ));
    }

    #[cfg(feature = "openai")]
    #[tokio::test]
    async fn openai_factory_honors_include_dynamic_false_without_static_fallback() {
        let config = config_without_credentials();
        let options = ModelListOptions {
            include_dynamic: false,
            ..Default::default()
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

        #[tokio::test]
        async fn missing_credentials_do_not_return_static_models() {
            let err = GitHubCopilotFactory
                .list_models(
                    &config_without_credentials(),
                    "github-copilot",
                    &ModelListOptions::default(),
                )
                .await
                .unwrap_err();
            assert!(matches!(err, RociError::MissingCredential { .. }));
        }

        #[tokio::test]
        async fn disabled_dynamic_discovery_is_empty_without_http() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/models"))
                .respond_with(ResponseTemplate::new(200))
                .expect(0)
                .mount(&server)
                .await;
            let config = config_with_copilot(server.uri());
            let catalog = GitHubCopilotFactory
                .list_models(
                    &config,
                    "github-copilot",
                    &ModelListOptions {
                        include_dynamic: false,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            assert!(catalog.models().is_empty());
        }

        #[tokio::test]
        async fn discovery_errors_do_not_return_static_models() {
            for status in [401, 403, 404, 503] {
                let server = MockServer::start().await;
                Mock::given(method("GET"))
                    .and(path("/models"))
                    .respond_with(ResponseTemplate::new(status))
                    .mount(&server)
                    .await;
                let config = config_with_copilot(server.uri());
                assert!(GitHubCopilotFactory
                    .list_models(&config, "github-copilot", &ModelListOptions::default())
                    .await
                    .is_err());
            }
        }

        #[tokio::test]
        async fn live_catalog_preserves_unrecognized_models() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/models"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"data":[{"id":"future-copilot-model"}]})),
                )
                .mount(&server)
                .await;
            let catalog = GitHubCopilotFactory
                .list_models(
                    &config_with_copilot(server.uri()),
                    "github-copilot",
                    &ModelListOptions::default(),
                )
                .await
                .unwrap();
            assert_eq!(catalog.models().len(), 1);
            assert_eq!(catalog.models()[0].model_id, "future-copilot-model");
            assert!(matches!(
                catalog.models()[0].source,
                ModelCatalogSource::Dynamic { .. }
            ));
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
                        provider_metadata: None,
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
            #[cfg(feature = "cursor")]
            ("cursor", "Cursor", &[], true),
            #[cfg(feature = "openai")]
            ("openai", "OpenAI", &[CredentialFlow::ApiKey], true),
            #[cfg(feature = "openai")]
            ("codex", "Codex", &[CredentialFlow::ApiKey], true),
            #[cfg(feature = "anthropic")]
            ("anthropic", "Anthropic", &[CredentialFlow::ApiKey], true),
            #[cfg(feature = "google")]
            ("google", "Google", &[CredentialFlow::ApiKey], true),
            #[cfg(feature = "grok")]
            ("grok", "Grok", &[CredentialFlow::ApiKey], true),
            #[cfg(feature = "groq")]
            ("groq", "Groq", &[CredentialFlow::ApiKey], true),
            #[cfg(feature = "mistral")]
            ("mistral", "Mistral", &[CredentialFlow::ApiKey], true),
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
            ("openrouter", "OpenRouter", &[CredentialFlow::ApiKey], true),
            #[cfg(feature = "together")]
            ("together", "Together", &[CredentialFlow::ApiKey], true),
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
