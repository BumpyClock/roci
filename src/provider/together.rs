//! Together AI provider (OpenAI-compatible).

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::RociError;
use crate::models::capabilities::ModelCapabilities;
use crate::types::TextStreamDelta;

use super::openai::OpenAiProvider;
use super::{ModelProvider, ProviderRequest, ProviderResponse};

pub struct TogetherProvider {
    inner: OpenAiProvider,
}

impl TogetherProvider {
    pub fn new(model_id: String, api_key: String) -> Self {
        let model = crate::models::openai::OpenAiModel::Custom(model_id);
        Self {
            inner: OpenAiProvider::new(
                model,
                api_key,
                Some("https://api.together.xyz/v1".to_string()),
            ),
        }
    }
}

#[async_trait]
impl ModelProvider for TogetherProvider {
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
