//! GitHub Copilot provider using Copilot auth/config keys.

use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};

use crate::error::RociError;
use crate::models::capabilities::ModelCapabilities;
use crate::types::TextStreamDelta;

use super::openai_compatible::OpenAiCompatibleProvider;
use super::{ModelProvider, ProviderRequest, ProviderResponse};

const COPILOT_EDITOR_VERSION: &str = "vscode/1.96.2";
const COPILOT_EDITOR_PLUGIN_VERSION: &str = "copilot-chat/0.26.7";
const COPILOT_INTEGRATION_ID: &str = "vscode-chat";
const COPILOT_USER_AGENT: &str = "GitHubCopilotChat/0.26.7";
const COPILOT_API_VERSION: &str = "2025-04-01";

pub struct GitHubCopilotProvider {
    inner: OpenAiCompatibleProvider,
}

impl GitHubCopilotProvider {
    pub fn new(model_id: String, api_key: String, base_url: String) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            "Editor-Version",
            HeaderValue::from_static(COPILOT_EDITOR_VERSION),
        );
        headers.insert(
            "Editor-Plugin-Version",
            HeaderValue::from_static(COPILOT_EDITOR_PLUGIN_VERSION),
        );
        headers.insert(
            "Copilot-Integration-Id",
            HeaderValue::from_static(COPILOT_INTEGRATION_ID),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static(COPILOT_USER_AGENT));
        headers.insert(
            "X-Github-Api-Version",
            HeaderValue::from_static(COPILOT_API_VERSION),
        );
        Self {
            inner: OpenAiCompatibleProvider::new_with_headers(model_id, api_key, base_url, headers),
        }
    }
}

#[async_trait]
impl ModelProvider for GitHubCopilotProvider {
    fn provider_name(&self) -> &str {
        "github-copilot"
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
