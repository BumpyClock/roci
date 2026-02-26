//! Azure OpenAI provider.

use async_trait::async_trait;
use futures::stream::BoxStream;

use roci_core::error::RociError;
use roci_core::models::capabilities::ModelCapabilities;
use roci_core::types::TextStreamDelta;

use super::openai::OpenAiProvider;
use roci_core::provider::{ModelProvider, ProviderRequest, ProviderResponse};

use crate::models::openai::OpenAiModel;

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
        let model = OpenAiModel::Custom(deployment);
        Self {
            inner: OpenAiProvider::new(model, api_key, Some(base_url), None),
        }
    }
}

#[async_trait]
impl ModelProvider for AzureOpenAiProvider {
    fn provider_name(&self) -> &str {
        "azure"
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
