//! Model provider trait and implementations.

pub mod format;
pub mod http;
pub mod schema;

#[cfg(feature = "openai")]
pub mod openai;
#[cfg(feature = "openai")]
pub mod openai_responses;

#[cfg(feature = "anthropic")]
pub mod anthropic;

#[cfg(feature = "google")]
pub mod google;

#[cfg(feature = "grok")]
pub mod grok;
#[cfg(feature = "groq")]
pub mod groq;
#[cfg(feature = "lmstudio")]
pub mod lmstudio;
#[cfg(feature = "mistral")]
pub mod mistral;
#[cfg(feature = "ollama")]
pub mod ollama;

#[cfg(feature = "anthropic-compatible")]
pub mod anthropic_compatible;
#[cfg(feature = "openai-compatible")]
pub mod openai_compatible;

#[cfg(feature = "azure")]
pub mod azure;
#[cfg(feature = "openrouter")]
pub mod openrouter;
#[cfg(feature = "replicate")]
pub mod replicate;
#[cfg(feature = "together")]
pub mod together;

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::RociError;
use crate::models::{capabilities::ModelCapabilities, LanguageModel};
use crate::types::{
    message::{AgentToolCall, ContentPart},
    FinishReason, GenerationSettings, ModelMessage, TextStreamDelta, Usage,
};

/// A request sent to a model provider.
#[derive(Debug, Clone)]
pub struct ProviderRequest {
    pub messages: Vec<ModelMessage>,
    pub settings: GenerationSettings,
    pub tools: Option<Vec<ToolDefinition>>,
    pub response_format: Option<crate::types::generation::ResponseFormat>,
}

/// Tool definition sent to the provider API.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Response from a provider.
#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub text: String,
    pub usage: Usage,
    pub tool_calls: Vec<AgentToolCall>,
    pub finish_reason: Option<FinishReason>,
    /// Thinking content blocks (Anthropic extended thinking).
    pub thinking: Vec<ContentPart>,
}

/// Core trait implemented by all model providers.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Provider name (e.g., "openai", "google").
    fn provider_name(&self) -> &str;
    /// The model ID this provider instance serves.
    fn model_id(&self) -> &str;

    /// Capabilities of the model.
    fn capabilities(&self) -> &ModelCapabilities;

    /// Generate text (non-streaming).
    async fn generate_text(&self, request: &ProviderRequest)
        -> Result<ProviderResponse, RociError>;

    /// Generate text (streaming).
    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError>;
}

/// Create a provider for the given model, using the provided config.
#[allow(unused_variables)]
pub fn create_provider(
    model: &LanguageModel,
    config: &crate::config::RociConfig,
) -> Result<Box<dyn ModelProvider>, RociError> {
    match model {
        #[cfg(feature = "openai")]
        LanguageModel::OpenAi(m) => {
            let api_key = config
                .get_api_key("openai")
                .ok_or_else(|| RociError::Authentication("Missing OPENAI_API_KEY".into()))?;
            if m.uses_responses_api() {
                Ok(Box::new(openai_responses::OpenAiResponsesProvider::new(
                    m.clone(),
                    api_key,
                    config.get_base_url("openai"),
                )))
            } else {
                Ok(Box::new(openai::OpenAiProvider::new(
                    m.clone(),
                    api_key,
                    config.get_base_url("openai"),
                )))
            }
        }
        #[cfg(feature = "openai")]
        LanguageModel::OpenAiCodex(m) => {
            let api_key = config
                .get_api_key("openai-codex")
                .ok_or_else(|| RociError::Authentication("Missing OPENAI_CODEX_TOKEN".into()))?;
            let base_url = config
                .get_base_url("openai-codex")
                .or_else(|| config.get_base_url("openai"));
            if m.uses_responses_api() {
                Ok(Box::new(openai_responses::OpenAiResponsesProvider::new(
                    m.clone(),
                    api_key,
                    base_url,
                )))
            } else {
                Ok(Box::new(openai::OpenAiProvider::new(
                    m.clone(),
                    api_key,
                    base_url,
                )))
            }
        }
        #[cfg(feature = "anthropic")]
        LanguageModel::Anthropic(m) => {
            let api_key = config
                .get_api_key("anthropic")
                .ok_or_else(|| RociError::Authentication("Missing ANTHROPIC_API_KEY".into()))?;
            Ok(Box::new(anthropic::AnthropicProvider::new(
                m.clone(),
                api_key,
                config.get_base_url("anthropic"),
            )))
        }
        #[cfg(feature = "google")]
        LanguageModel::Google(m) => {
            let api_key = config
                .get_api_key("google")
                .ok_or_else(|| RociError::Authentication("Missing GOOGLE_API_KEY".into()))?;
            Ok(Box::new(google::GoogleProvider::new(m.clone(), api_key)))
        }
        #[cfg(feature = "grok")]
        LanguageModel::Grok(m) => {
            let api_key = config
                .get_api_key("grok")
                .or_else(|| config.get_api_key("xai"))
                .ok_or_else(|| RociError::Authentication("Missing XAI_API_KEY".into()))?;
            Ok(Box::new(grok::GrokProvider::new(m.clone(), api_key)))
        }
        #[cfg(feature = "groq")]
        LanguageModel::Groq(m) => {
            let api_key = config
                .get_api_key("groq")
                .ok_or_else(|| RociError::Authentication("Missing GROQ_API_KEY".into()))?;
            Ok(Box::new(groq::GroqProvider::new(m.clone(), api_key)))
        }
        #[cfg(feature = "mistral")]
        LanguageModel::Mistral(m) => {
            let api_key = config
                .get_api_key("mistral")
                .ok_or_else(|| RociError::Authentication("Missing MISTRAL_API_KEY".into()))?;
            Ok(Box::new(mistral::MistralProvider::new(m.clone(), api_key)))
        }
        #[cfg(feature = "ollama")]
        LanguageModel::Ollama(m) => {
            let base_url = config
                .get_base_url("ollama")
                .unwrap_or_else(|| "http://localhost:11434".to_string());
            Ok(Box::new(ollama::OllamaProvider::new(m.clone(), base_url)))
        }
        #[cfg(feature = "lmstudio")]
        LanguageModel::LmStudio(m) => {
            let base_url = config
                .get_base_url("lmstudio")
                .unwrap_or_else(|| "http://localhost:1234".to_string());
            Ok(Box::new(lmstudio::LmStudioProvider::new(
                m.clone(),
                base_url,
            )))
        }
        #[cfg(feature = "openai-compatible")]
        LanguageModel::OpenAiCompatible(m) => {
            let api_key = config
                .get_api_key("openai-compatible")
                .or_else(|| config.get_api_key("openai"))
                .ok_or_else(|| RociError::Authentication("Missing OPENAI_COMPAT_API_KEY".into()))?;
            let base_url = m
                .base_url
                .clone()
                .or_else(|| config.get_base_url("openai-compatible"))
                .or_else(|| config.get_base_url("openai"))
                .ok_or_else(|| RociError::Configuration("Missing OPENAI_COMPAT_BASE_URL".into()))?;
            Ok(Box::new(openai_compatible::OpenAiCompatibleProvider::new(
                m.model_id.clone(),
                api_key,
                base_url,
            )))
        }
        LanguageModel::Custom { provider, .. } => Err(RociError::ModelNotFound(format!(
            "No built-in provider for '{provider}'. Use openai_compatible or anthropic_compatible."
        ))),
        #[allow(unreachable_patterns)]
        _ => Err(RociError::ModelNotFound(format!(
            "Provider for model '{}' not enabled via feature flags",
            model
        ))),
    }
}
