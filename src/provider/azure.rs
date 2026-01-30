//! Azure OpenAI provider.

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::RociError;
use crate::models::capabilities::ModelCapabilities;
use crate::types::TextStreamDelta;

use super::openai::OpenAiProvider;
use super::{ModelProvider, ProviderRequest, ProviderResponse};

/// Azure OpenAI Service provider.
pub struct AzureOpenAiProvider {
    inner: OpenAiProvider,
}

impl AzureOpenAiProvider {
    /// Create with Azure-specific endpoint.
    /// `endpoint`: e.g., "https://myresource.openai.azure.com"
    /// `deployment`: e.g., "gpt-4o"
    /// `api_version`: e.g., "2024-06-01"
    pub fn new(endpoint: String, deployment: String, api_key: String, api_version: String) -> Self {
        let base_url = format!(
            "{}/openai/deployments/{}/chat/completions?api-version={}",
            endpoint.trim_end_matches('/'),
            deployment,
            api_version
        );
        let model = crate::models::openai::OpenAiModel::Custom(deployment);
        Self {
            inner: OpenAiProvider::new(model, api_key, Some(base_url)),
        }
    }
}

#[async_trait]
impl ModelProvider for AzureOpenAiProvider {
    fn model_id(&self) -> &str { self.inner.model_id() }
    fn capabilities(&self) -> &ModelCapabilities { self.inner.capabilities() }
    async fn generate_text(&self, request: &ProviderRequest) -> Result<ProviderResponse, RociError> {
        self.inner.generate_text(request).await
    }
    async fn stream_text(&self, request: &ProviderRequest) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        self.inner.stream_text(request).await
    }
}
