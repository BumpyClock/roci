//! Generic OpenAI-compatible provider.

use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::header::HeaderMap;

use crate::error::RociError;
use crate::models::capabilities::ModelCapabilities;
use crate::types::TextStreamDelta;

use super::openai::OpenAiProvider;
use super::{ModelProvider, ProviderRequest, ProviderResponse};

/// Generic provider for any OpenAI-compatible API.
pub struct OpenAiCompatibleProvider {
    inner: OpenAiProvider,
}

impl OpenAiCompatibleProvider {
    pub fn new(model_id: String, api_key: String, base_url: String) -> Self {
        Self::new_with_headers(model_id, api_key, base_url, HeaderMap::new())
    }

    pub fn new_with_headers(
        model_id: String,
        api_key: String,
        base_url: String,
        extra_headers: HeaderMap,
    ) -> Self {
        let model = crate::models::openai::OpenAiModel::Custom(model_id);
        Self {
            inner: OpenAiProvider::new_with_extra_headers(
                model,
                api_key,
                Some(base_url),
                None,
                extra_headers,
            ),
        }
    }
}

#[async_trait]
impl ModelProvider for OpenAiCompatibleProvider {
    fn provider_name(&self) -> &str {
        "openai-compatible"
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
