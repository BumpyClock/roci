//! Model provider trait, registry, and shared utilities.

pub mod factory;
pub mod format;
pub mod http;
pub mod registry;
pub mod sanitize;
pub mod schema;

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::RociError;
use crate::models::capabilities::ModelCapabilities;
use crate::types::{
    message::{AgentToolCall, ContentPart},
    FinishReason, GenerationSettings, ModelMessage, TextStreamDelta, Usage,
};

pub use factory::ProviderFactory;
pub use registry::ProviderRegistry;
pub use sanitize::sanitize_messages_for_provider;

pub const TRANSPORT_DIRECT: &str = "direct";
pub const TRANSPORT_PROXY: &str = "proxy";
pub const SUPPORTED_TRANSPORTS: [&str; 2] = [TRANSPORT_DIRECT, TRANSPORT_PROXY];

/// A request sent to a model provider.
#[derive(Debug, Clone)]
pub struct ProviderRequest {
    pub messages: Vec<ModelMessage>,
    pub settings: GenerationSettings,
    pub tools: Option<Vec<ToolDefinition>>,
    pub response_format: Option<crate::types::generation::ResponseFormat>,
    /// Optional session ID for provider-side prompt caching and session affinity.
    pub session_id: Option<String>,
    /// Optional transport preference supplied by runtime/loop orchestration.
    ///
    /// Supported values are `"direct"` and `"proxy"`.
    /// Unsupported values are rejected by the runner before provider execution.
    pub transport: Option<String>,
}

pub fn validate_transport_preference(transport: Option<&str>) -> Result<(), RociError> {
    if let Some(value) = transport {
        if !SUPPORTED_TRANSPORTS.contains(&value) {
            return Err(RociError::InvalidArgument(format!(
                "unsupported provider transport '{value}' (supported: {})",
                SUPPORTED_TRANSPORTS.join(", ")
            )));
        }
    }
    Ok(())
}

/// Tool definition sent to the provider API.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Response from a provider.
#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub text: String,
    pub usage: Usage,
    pub tool_calls: Vec<AgentToolCall>,
    pub finish_reason: Option<FinishReason>,
    /// Thinking content blocks (Anthropic extended thinking).
    pub thinking: Vec<ContentPart>,
}

/// Core trait implemented by all model providers.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Provider name (e.g., "openai", "google").
    fn provider_name(&self) -> &str;
    /// The model ID this provider instance serves.
    fn model_id(&self) -> &str;

    /// Capabilities of the model.
    fn capabilities(&self) -> &ModelCapabilities;

    /// Generate text (non-streaming).
    async fn generate_text(&self, request: &ProviderRequest)
        -> Result<ProviderResponse, RociError>;

    /// Generate text (streaming).
    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError>;
}

/// Resolve an API key from config for the given provider, returning an
/// authentication error with the specified message on failure.
pub fn require_api_key(
    config: &crate::config::RociConfig,
    provider: crate::models::ProviderKey,
    missing_message: &'static str,
) -> Result<String, RociError> {
    config
        .get_api_key_for(provider)
        .ok_or_else(|| RociError::Authentication(missing_message.to_string()))
}
