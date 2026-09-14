//! Text generation without tool execution.

use tracing::debug;

use crate::error::RociError;
use crate::provider::{ModelProvider, ProviderRequest};
use crate::types::*;

/// Generate text with no tool execution.
pub async fn generate_text(
    provider: &dyn ModelProvider,
    messages: Vec<ModelMessage>,
    settings: GenerationSettings,
) -> Result<GenerateTextResult, RociError> {
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
    Ok(GenerateTextResult {
        text: response.text,
        tool_calls: response.tool_calls,
        messages,
        usage: response.usage,
        finish_reason: response.finish_reason,
    })
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use futures::stream::BoxStream;

    use super::*;
    use crate::models::ModelCapabilities;
    use crate::provider::ProviderResponse;

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
            request: &ProviderRequest,
        ) -> Result<ProviderResponse, RociError> {
            assert_eq!(request.messages.len(), 1);
            assert_eq!(request.messages[0].text(), "hello");
            assert_eq!(request.settings.temperature, Some(0.25));
            assert_eq!(request.settings.max_tokens, Some(80));
            assert!(matches!(
                request.response_format,
                Some(ResponseFormat::JsonObject)
            ));
            assert!(request.tools.is_none());
            Ok(ProviderResponse {
                text: r#"{"answer":42}"#.into(),
                usage: Usage {
                    input_tokens: 3,
                    output_tokens: 5,
                    total_tokens: 8,
                    ..Usage::default()
                },
                tool_calls: vec![AgentToolCall {
                    id: "unexpected-call".into(),
                    name: "lookup".into(),
                    arguments: serde_json::json!({"item": "answer"}),
                    called_as: None,
                    recipient: None,
                }],
                finish_reason: Some(FinishReason::Stop),
                thinking: Vec::new(),
            })
        }

        async fn stream_text(
            &self,
            _request: &ProviderRequest,
        ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
            panic!("stream should not be called")
        }
    }

    #[tokio::test]
    async fn generate_text_preserves_request_and_response() {
        let result = generate_text(
            &StubProvider,
            vec![ModelMessage::user("hello")],
            GenerationSettings {
                temperature: Some(0.25),
                max_tokens: Some(80),
                response_format: Some(ResponseFormat::JsonObject),
                ..GenerationSettings::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(result.text, r#"{"answer":42}"#);
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].text(), "hello");
        assert_eq!(
            result.usage,
            Usage {
                input_tokens: 3,
                output_tokens: 5,
                total_tokens: 8,
                ..Usage::default()
            }
        );
        assert_eq!(result.finish_reason, Some(FinishReason::Stop));
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].id, "unexpected-call");
        assert_eq!(result.tool_calls[0].name, "lookup");
        assert_eq!(
            result.tool_calls[0].arguments,
            serde_json::json!({"item": "answer"})
        );
    }
}
