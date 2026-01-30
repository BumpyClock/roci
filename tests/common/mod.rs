//! Shared test helpers and mock provider.

use async_trait::async_trait;
use futures::stream::BoxStream;

use roci::error::RociError;
use roci::models::capabilities::ModelCapabilities;
use roci::provider::{ModelProvider, ProviderRequest, ProviderResponse};
use roci::types::*;

/// A mock provider that returns canned responses.
pub struct MockProvider {
    model_id: String,
    capabilities: ModelCapabilities,
    responses: std::sync::Mutex<Vec<ProviderResponse>>,
}

impl MockProvider {
    pub fn new(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            capabilities: ModelCapabilities::full(128_000),
            responses: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Queue a text response.
    pub fn queue_response(&self, text: &str) {
        self.responses.lock().unwrap().push(ProviderResponse {
            text: text.to_string(),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 20,
                total_tokens: 30,
                ..Default::default()
            },
            tool_calls: vec![],
            finish_reason: Some(FinishReason::Stop),
        });
    }

    /// Queue a tool call response.
    pub fn queue_tool_call(&self, id: &str, name: &str, args: serde_json::Value) {
        self.responses.lock().unwrap().push(ProviderResponse {
            text: String::new(),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
                ..Default::default()
            },
            tool_calls: vec![message::AgentToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments: args,
                recipient: None,
            }],
            finish_reason: Some(FinishReason::ToolCalls),
        });
    }
}

#[async_trait]
impl ModelProvider for MockProvider {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            return Ok(ProviderResponse {
                text: "Mock response".to_string(),
                usage: Usage::default(),
                tool_calls: vec![],
                finish_reason: Some(FinishReason::Stop),
            });
        }
        Ok(responses.remove(0))
    }

    async fn stream_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        let text = {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                "Mock streamed response".to_string()
            } else {
                responses.remove(0).text
            }
        };

        let stream = async_stream::stream! {
            for chunk in text.chars().collect::<Vec<_>>().chunks(5) {
                let text: String = chunk.iter().collect();
                yield Ok(TextStreamDelta {
                    text,
                    event_type: StreamEventType::TextDelta,
                    tool_call: None,
                    finish_reason: None,
                    usage: None,
                });
            }
            yield Ok(TextStreamDelta {
                text: String::new(),
                event_type: StreamEventType::Done,
                tool_call: None,
                finish_reason: Some(FinishReason::Stop),
                usage: Some(Usage { input_tokens: 10, output_tokens: 20, total_tokens: 30, ..Default::default() }),
            });
        };

        Ok(Box::pin(stream))
    }
}
