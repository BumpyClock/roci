//! Ollama local provider (OpenAI-compatible).

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::RociError;
use crate::models::capabilities::ModelCapabilities;
use crate::models::ollama::OllamaModel;
use crate::types::TextStreamDelta;

use super::openai::OpenAiProvider;
use super::{ModelProvider, ProviderRequest, ProviderResponse};

pub struct OllamaProvider {
    inner: OpenAiProvider,
}

impl OllamaProvider {
    pub fn new(model: OllamaModel, base_url: String) -> Self {
        let openai_model = crate::models::openai::OpenAiModel::Custom(model.as_str().to_string());
        Self {
            inner: OpenAiProvider::new(
                openai_model,
                String::new(), // no API key for local
                Some(format!("{}/v1", base_url.trim_end_matches('/'))),
            ),
        }
    }
}

#[async_trait]
impl ModelProvider for OllamaProvider {
    fn provider_name(&self) -> &str {
        "ollama"
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
