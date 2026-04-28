//! Streaming text generation with stop conditions.

use futures::stream::BoxStream;
use futures::StreamExt;

use crate::error::RociError;
use crate::provider::{ModelProvider, ProviderRequest};
use crate::stop::StopCondition;
use crate::tools::tool::Tool;
use crate::types::*;

/// Stream text from a model, applying optional stop conditions.
///
/// Returns a stream of text deltas. Stop conditions can halt the stream early.
pub async fn stream_text(
    provider: std::sync::Arc<dyn ModelProvider>,
    messages: Vec<ModelMessage>,
    settings: GenerationSettings,
    stop_conditions: Vec<Box<dyn StopCondition>>,
) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
    stream_text_with_tools(provider, messages, settings, &[], stop_conditions).await
}

/// Stream text from a model with stop conditions.
pub async fn stream_text_with_tools(
    provider: std::sync::Arc<dyn ModelProvider>,
    messages: Vec<ModelMessage>,
    settings: GenerationSettings,
    tools: &[std::sync::Arc<dyn Tool>],
    stop_conditions: Vec<Box<dyn StopCondition>>,
) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
    if !tools.is_empty() {
        return Err(RociError::UnsupportedOperation(
            "generation::stream_text_with_tools does not execute tools; use Agent, AgentRuntime, or agent_loop::LoopRunner for tool-capable streams".to_string(),
        ));
    }

    let stream = async_stream::stream! {
        let mut accumulated_text = String::new();
        for cond in &stop_conditions {
            cond.reset().await;
        }

        let request = ProviderRequest {
            messages,
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
        let mut inner = match provider.stream_text(&request).await {
            Ok(stream) => stream,
            Err(e) => {
                yield Err(e);
                return;
            }
        };
        while let Some(item) = inner.next().await {
            match item {
                Ok(delta) => {
                    let event_type = delta.event_type;
                    let delta_text = delta.text.clone();
                    if !delta_text.is_empty() {
                        accumulated_text.push_str(&delta_text);
                    }
                    yield Ok(delta);
                    if matches!(event_type, StreamEventType::TextDelta) {
                        let mut stop_triggered = false;
                        for cond in &stop_conditions {
                            if cond.should_stop(&accumulated_text, Some(&delta_text)).await {
                                stop_triggered = true;
                                break;
                            }
                        }
                        if stop_triggered {
                            yield Ok(TextStreamDelta {
                                text: String::new(),
                                event_type: StreamEventType::Done,
                                tool_call: None,
                                finish_reason: Some(FinishReason::Stop),
                                usage: None,
                                reasoning: None,
                                reasoning_signature: None,
                                reasoning_type: None,
                            });
                            break;
                        }
                    }
                }
                Err(e) => {
                    yield Err(e);
                    return;
                }
            }
        }
    };
    Ok(Box::pin(stream))
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
            panic!("generate should not be called")
        }

        async fn stream_text(
            &self,
            _request: &ProviderRequest,
        ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
            panic!("provider should not be called when tools are supplied")
        }
    }

    #[tokio::test]
    async fn stream_text_with_tools_rejects_tools() {
        let tool = Arc::new(AgentTool::new(
            "lookup",
            "lookup",
            AgentToolParameters::empty(),
            |_args, _ctx| async { Ok(serde_json::json!({ "ok": true })) },
        ));
        let result = stream_text_with_tools(
            Arc::new(StubProvider),
            vec![ModelMessage::user("hello")],
            GenerationSettings::default(),
            &[tool],
            Vec::new(),
        )
        .await;
        let err = match result {
            Err(err) => err,
            Ok(_) => panic!("tools should be rejected"),
        };

        assert!(
            matches!(err, RociError::UnsupportedOperation(message) if message.contains("AgentRuntime"))
        );
    }
}
