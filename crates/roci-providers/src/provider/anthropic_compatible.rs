//! Generic Anthropic-compatible provider.

use async_trait::async_trait;
use futures::stream::BoxStream;

use roci_core::error::RociError;
use roci_core::models::capabilities::ModelCapabilities;
use roci_core::types::TextStreamDelta;

use super::anthropic::AnthropicProvider;
use roci_core::provider::{ModelProvider, ProviderRequest, ProviderResponse};

use crate::models::anthropic::AnthropicModel;

/// Generic provider for any Anthropic-compatible API.
pub struct AnthropicCompatibleProvider {
    inner: AnthropicProvider,
}

impl AnthropicCompatibleProvider {
    pub fn new(model_id: String, api_key: String, base_url: String) -> Self {
        let model = AnthropicModel::Custom(model_id);
        Self {
            inner: AnthropicProvider::new(model, api_key, Some(base_url)),
        }
    }
}

#[async_trait]
impl ModelProvider for AnthropicCompatibleProvider {
    fn provider_name(&self) -> &str {
        "anthropic-compatible"
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
