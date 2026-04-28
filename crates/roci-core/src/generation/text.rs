//! Text generation without tool execution.

use tracing::debug;

use crate::error::RociError;
use crate::provider::{ModelProvider, ProviderRequest};
use crate::tools::tool::Tool;
use crate::types::*;

/// Generate text with no tool execution.
pub async fn generate_text(
    provider: &dyn ModelProvider,
    messages: Vec<ModelMessage>,
    settings: GenerationSettings,
    tools: &[std::sync::Arc<dyn Tool>],
) -> Result<GenerateTextResult, RociError> {
    if !tools.is_empty() {
        return Err(RociError::UnsupportedOperation(
            "generation::generate_text does not execute tools; use Agent, AgentRuntime, or agent_loop::LoopRunner for tool-capable runs".to_string(),
        ));
    }

    let request = ProviderRequest {
        messages: messages.clone(),
        settings: settings.clone(),
        tools: None,
        response_format: settings.response_format.clone(),
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    debug!("generate_text: calling provider");
    let response = provider.generate_text(&request).await?;
    let step = GenerationStep {
        text: response.text.clone(),
        tool_calls: response.tool_calls.clone(),
        tool_results: Vec::new(),
        usage: response.usage.clone(),
        finish_reason: response.finish_reason,
    };
    Ok(GenerateTextResult {
        text: response.text,
        steps: vec![step],
        messages,
        usage: response.usage,
        finish_reason: response.finish_reason,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use futures::stream::BoxStream;

    use super::*;
    use crate::models::ModelCapabilities;
    use crate::provider::ProviderResponse;
    use crate::tools::{AgentTool, AgentToolParameters};

    struct StubProvider;

    #[async_trait]
    impl ModelProvider for StubProvider {
        fn provider_name(&self) -> &str {
            "stub"
        }

        fn model_id(&self) -> &str {
            "model"
        }

        fn capabilities(&self) -> &ModelCapabilities {
            static CAPABILITIES: std::sync::OnceLock<ModelCapabilities> =
                std::sync::OnceLock::new();
            CAPABILITIES.get_or_init(ModelCapabilities::default)
        }

        async fn generate_text(
            &self,
            _request: &ProviderRequest,
        ) -> Result<ProviderResponse, RociError> {
            panic!("provider should not be called when tools are supplied")
        }

        async fn stream_text(
            &self,
            _request: &ProviderRequest,
        ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
            panic!("stream should not be called")
        }
    }

    #[tokio::test]
    async fn generate_text_rejects_tools() {
        let tool = Arc::new(AgentTool::new(
            "lookup",
            "lookup",
            AgentToolParameters::empty(),
            |_args, _ctx| async { Ok(serde_json::json!({ "ok": true })) },
        ));
        let err = generate_text(
            &StubProvider,
            vec![ModelMessage::user("hello")],
            GenerationSettings::default(),
            &[tool],
        )
        .await
        .expect_err("tools should be rejected");

        assert!(
            matches!(err, RociError::UnsupportedOperation(message) if message.contains("AgentRuntime"))
        );
    }
}
