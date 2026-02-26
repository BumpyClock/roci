//! Grok provider (OpenAI-compatible).

use async_trait::async_trait;
use futures::stream::BoxStream;

use roci_core::error::RociError;
use roci_core::models::capabilities::ModelCapabilities;
use roci_core::types::TextStreamDelta;

use super::openai::OpenAiProvider;
use roci_core::provider::{ModelProvider, ProviderRequest, ProviderResponse};

use crate::models::grok::GrokModel;
use crate::models::openai::OpenAiModel;

pub struct GrokProvider {
    inner: OpenAiProvider,
}

impl GrokProvider {
    pub fn new(model: GrokModel, api_key: String) -> Self {
        let openai_model = OpenAiModel::Custom(model.as_str().to_string());
        Self {
            inner: OpenAiProvider::new(
                openai_model,
                api_key,
                Some("https://api.x.ai/v1".to_string()),
                None,
            ),
        }
    }
}

#[async_trait]
impl ModelProvider for GrokProvider {
    fn provider_name(&self) -> &str {
        "grok"
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
