//! LMStudio local provider (OpenAI-compatible).

use async_trait::async_trait;
use futures::stream::BoxStream;

use roci_core::error::RociError;
use roci_core::models::capabilities::ModelCapabilities;
use roci_core::types::TextStreamDelta;

use super::openai::OpenAiProvider;
use roci_core::provider::{ModelProvider, ProviderRequest, ProviderResponse};

use crate::models::lmstudio::LmStudioModel;
use crate::models::openai::OpenAiModel;

pub struct LmStudioProvider {
    inner: OpenAiProvider,
    capabilities: ModelCapabilities,
}

impl LmStudioProvider {
    pub fn new(model: LmStudioModel, base_url: String) -> Self {
        let capabilities = model.capabilities();
        let openai_model = OpenAiModel::Custom(model.as_str().to_string());
        Self {
            inner: OpenAiProvider::new_without_auth(
                openai_model,
                Some(format!("{}/v1", base_url.trim_end_matches('/'))),
            ),
            capabilities,
        }
    }
}

#[async_trait]
impl ModelProvider for LmStudioProvider {
    fn provider_name(&self) -> &str {
        "lmstudio"
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
    fn lmstudio_provider_uses_lmstudio_model_capabilities() {
        let provider = LmStudioProvider::new(
            LmStudioModel::Custom("loaded-model".to_string()),
            "http://127.0.0.1:1234".to_string(),
        );
        let caps = provider.capabilities();

        assert!(!caps.supports_vision);
        assert!(caps.input.image.is_none());
        assert_eq!(caps.supports_vision, caps.input.image.is_some());
    }
}
