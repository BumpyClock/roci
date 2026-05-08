//! Mistral provider (OpenAI-compatible).

use async_trait::async_trait;
use futures::stream::BoxStream;

use roci_core::error::RociError;
use roci_core::models::capabilities::ModelCapabilities;
use roci_core::types::TextStreamDelta;

use super::openai::OpenAiProvider;
use roci_core::provider::{ModelProvider, ProviderRequest, ProviderResponse};

use crate::models::mistral::MistralModel;
use crate::models::openai::OpenAiModel;

pub struct MistralProvider {
    inner: OpenAiProvider,
    capabilities: ModelCapabilities,
}

impl MistralProvider {
    pub fn new(model: MistralModel, api_key: String) -> Self {
        let capabilities = model.capabilities();
        let openai_model = OpenAiModel::Custom(model.as_str().to_string());
        Self {
            inner: OpenAiProvider::new(
                openai_model,
                api_key,
                Some("https://api.mistral.ai/v1".to_string()),
                None,
            ),
            capabilities,
        }
    }
}

#[async_trait]
impl ModelProvider for MistralProvider {
    fn provider_name(&self) -> &str {
        "mistral"
    }
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }
    async fn generate_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        self.inner.generate_text(request).await
    }
    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        self.inner.stream_text(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mistral_large_provider_supports_image_input() {
        let provider = MistralProvider::new(MistralModel::MistralLarge, String::new());
        let caps = provider.capabilities();

        assert!(caps.supports_vision);
        assert!(caps.input.image.is_some());
        assert_eq!(caps.supports_vision, caps.input.image.is_some());
    }

    #[test]
    fn mistral_medium_provider_supports_image_input() {
        let provider = MistralProvider::new(MistralModel::MistralMedium, String::new());
        let caps = provider.capabilities();

        assert!(caps.supports_vision);
        assert!(caps.input.image.is_some());
        assert_eq!(caps.supports_vision, caps.input.image.is_some());
    }

    #[test]
    fn mistral_small_provider_supports_image_input() {
        let provider = MistralProvider::new(MistralModel::MistralSmall, String::new());
        let caps = provider.capabilities();

        assert!(caps.supports_vision);
        assert!(caps.input.image.is_some());
        assert_eq!(caps.supports_vision, caps.input.image.is_some());
    }

    #[test]
    fn codestral_provider_is_text_only() {
        let provider = MistralProvider::new(MistralModel::Codestral, String::new());
        let caps = provider.capabilities();

        assert!(!caps.supports_vision);
        assert!(caps.input.image.is_none());
        assert_eq!(caps.supports_vision, caps.input.image.is_some());
    }
}
