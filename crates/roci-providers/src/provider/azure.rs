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
            "{}/openai/deployments/{}",
            endpoint.trim_end_matches('/'),
            deployment,
        );
        let extra_query = format!("api-version={}", api_version);
        let model = OpenAiModel::Custom(deployment);
        Self {
            inner: OpenAiProvider::new_with_api_key_auth(
                model,
                api_key,
                Some(base_url),
                None,
                reqwest::header::HeaderMap::new(),
                Some(extra_query),
            ),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn azure_url_does_not_double_append_chat_completions() {
        let provider = AzureOpenAiProvider::new(
            "https://myresource.openai.azure.com".to_string(),
            "gpt-4o".to_string(),
            "test-key".to_string(),
            "2024-06-01".to_string(),
        );
        let url = provider.inner.chat_url();
        assert_eq!(
            url,
            "https://myresource.openai.azure.com/openai/deployments/gpt-4o/chat/completions?api-version=2024-06-01"
        );
    }

    #[test]
    fn azure_url_trims_trailing_slash_from_endpoint() {
        let provider = AzureOpenAiProvider::new(
            "https://myresource.openai.azure.com/".to_string(),
            "gpt-4o".to_string(),
            "test-key".to_string(),
            "2024-06-01".to_string(),
        );
        let url = provider.inner.chat_url();
        assert!(
            !url.contains("//openai"),
            "trailing slash should be trimmed: {url}"
        );
        assert!(
            url.contains("/openai/deployments/gpt-4o/chat/completions?api-version="),
            "url should have correct path: {url}"
        );
    }

    #[test]
    fn azure_uses_api_key_header_not_authorization_bearer() {
        let provider = AzureOpenAiProvider::new(
            "https://myresource.openai.azure.com".to_string(),
            "gpt-4o".to_string(),
            "test-key".to_string(),
            "2024-06-01".to_string(),
        );
        let headers = provider.inner.build_headers();
        assert_eq!(
            headers.get("api-key").and_then(|value| value.to_str().ok()),
            Some("test-key")
        );
        assert!(headers.get(reqwest::header::AUTHORIZATION).is_none());
    }
}
