//! Together AI provider (OpenAI-compatible).

use async_trait::async_trait;
use futures::stream::BoxStream;

use roci_core::error::RociError;
use roci_core::models::capabilities::ModelCapabilities;
use roci_core::types::TextStreamDelta;

use super::openai::OpenAiProvider;
use roci_core::provider::{ModelProvider, ProviderRequest, ProviderResponse};

use crate::models::openai::OpenAiModel;

pub struct TogetherProvider {
    inner: OpenAiProvider,
}

impl TogetherProvider {
    pub fn new(model_id: String, api_key: String) -> Self {
        let model = OpenAiModel::Custom(model_id);
        Self {
            inner: OpenAiProvider::new(
                model,
                api_key,
                Some("https://api.together.xyz/v1".to_string()),
                None,
            ),
        }
    }
}

#[async_trait]
impl ModelProvider for TogetherProvider {
    fn provider_name(&self) -> &str {
        "together"
    }
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn capabilities(&self) -> &ModelCapabilities {
        self.inner.capabilities()
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
