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
}

impl LmStudioProvider {
    pub fn new(model: LmStudioModel, base_url: String) -> Self {
        let openai_model = OpenAiModel::Custom(model.as_str().to_string());
        Self {
            inner: OpenAiProvider::new(
                openai_model,
                String::new(),
                Some(format!("{}/v1", base_url.trim_end_matches('/'))),
                None,
            ),
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
