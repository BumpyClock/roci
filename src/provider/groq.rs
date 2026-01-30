//! Groq provider (OpenAI-compatible).

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::RociError;
use crate::models::capabilities::ModelCapabilities;
use crate::models::groq::GroqModel;
use crate::types::TextStreamDelta;

use super::openai::OpenAiProvider;
use super::{ModelProvider, ProviderRequest, ProviderResponse};

pub struct GroqProvider {
    inner: OpenAiProvider,
}

impl GroqProvider {
    pub fn new(model: GroqModel, api_key: String) -> Self {
        let openai_model = crate::models::openai::OpenAiModel::Custom(model.as_str().to_string());
        Self {
            inner: OpenAiProvider::new(
                openai_model,
                api_key,
                Some("https://api.groq.com/openai/v1".to_string()),
            ),
        }
    }
}

#[async_trait]
impl ModelProvider for GroqProvider {
    fn model_id(&self) -> &str { self.inner.model_id() }
    fn capabilities(&self) -> &ModelCapabilities { self.inner.capabilities() }
    async fn generate_text(&self, request: &ProviderRequest) -> Result<ProviderResponse, RociError> {
        self.inner.generate_text(request).await
    }
    async fn stream_text(&self, request: &ProviderRequest) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        self.inner.stream_text(request).await
    }
}
