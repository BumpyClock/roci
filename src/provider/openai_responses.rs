//! OpenAI Responses API provider (for GPT-5, o3, o4-mini, etc.)

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use serde::Deserialize;
use tracing::debug;

use crate::error::RociError;
use crate::models::capabilities::ModelCapabilities;
use crate::models::openai::OpenAiModel;
use crate::types::*;

use super::http::{bearer_headers, shared_client};
use super::{ModelProvider, ProviderRequest, ProviderResponse};

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

pub struct OpenAiResponsesProvider {
    model: OpenAiModel,
    api_key: String,
    base_url: String,
    capabilities: ModelCapabilities,
}

impl OpenAiResponsesProvider {
    pub fn new(model: OpenAiModel, api_key: String, base_url: Option<String>) -> Self {
        let capabilities = model.capabilities();
        Self {
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            model,
            api_key,
            capabilities,
        }
    }

    fn build_request_body(&self, request: &ProviderRequest, stream: bool) -> serde_json::Value {
        // Convert messages to Responses API "input" format
        let input: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::Tool => "tool",
                };
                serde_json::json!({
                    "role": role,
                    "content": m.text(),
                })
            })
            .collect();

        let mut body = serde_json::json!({
            "model": self.model.as_str(),
            "input": input,
            "stream": stream,
        });

        let obj = body.as_object_mut().unwrap();

        if let Some(max) = request.settings.max_tokens {
            obj.insert("max_output_tokens".into(), max.into());
        }
        if let Some(temp) = request.settings.temperature {
            obj.insert("temperature".into(), temp.into());
        }
        if let Some(top_p) = request.settings.top_p {
            obj.insert("top_p".into(), top_p.into());
        }

        if let Some(ref effort) = request.settings.reasoning_effort {
            obj.insert(
                "reasoning".into(),
                serde_json::json!({ "effort": effort.to_string() }),
            );
        }

        if let Some(ref tools) = request.tools {
            if !tools.is_empty() {
                let tool_defs: Vec<serde_json::Value> = tools
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "type": "function",
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        })
                    })
                    .collect();
                obj.insert("tools".into(), tool_defs.into());
            }
        }

        body
    }
}

#[async_trait]
impl ModelProvider for OpenAiResponsesProvider {
    fn model_id(&self) -> &str {
        self.model.as_str()
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(&self, request: &ProviderRequest) -> Result<ProviderResponse, RociError> {
        let body = self.build_request_body(request, false);
        let url = format!("{}/responses", self.base_url);

        debug!(model = self.model.as_str(), "OpenAI Responses generate_text");

        let resp = shared_client()
            .post(&url)
            .headers(bearer_headers(&self.api_key))
            .json(&body)
            .send()
            .await?;

        let status = resp.status().as_u16();
        if status != 200 {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(super::http::status_to_error(status, &body_text));
        }

        let data: ResponsesApiResponse = resp.json().await?;

        let mut text = String::new();
        let mut tool_calls = Vec::new();

        for item in &data.output {
            if let Some(content) = &item.content {
                for c in content {
                    if let Some(ref t) = c.text {
                        text.push_str(t);
                    }
                }
            }
            if item.r#type == "function_call" {
                if let (Some(ref id), Some(ref name), Some(ref args)) =
                    (&item.call_id, &item.name, &item.arguments)
                {
                    tool_calls.push(message::AgentToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: serde_json::from_str(args)
                            .unwrap_or(serde_json::Value::String(args.clone())),
                    });
                }
            }
        }

        let finish_reason = if !tool_calls.is_empty() {
            Some(FinishReason::ToolCalls)
        } else {
            data.status.as_deref().and_then(|s| match s {
                "completed" => Some(FinishReason::Stop),
                "incomplete" => Some(FinishReason::Length),
                _ => None,
            })
        };

        Ok(ProviderResponse {
            text,
            usage: data.usage.map(|u| Usage {
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                total_tokens: u.input_tokens + u.output_tokens,
                ..Default::default()
            }).unwrap_or_default(),
            tool_calls,
            finish_reason,
        })
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        let body = self.build_request_body(request, true);
        let url = format!("{}/responses", self.base_url);

        debug!(model = self.model.as_str(), "OpenAI Responses stream_text");

        let resp = shared_client()
            .post(&url)
            .headers(bearer_headers(&self.api_key))
            .json(&body)
            .send()
            .await?;

        let status = resp.status().as_u16();
        if status != 200 {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(super::http::status_to_error(status, &body_text));
        }

        let byte_stream = resp.bytes_stream();

        let stream = async_stream::stream! {
            let mut buffer = String::new();
            futures::pin_mut!(byte_stream);

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        yield Err(RociError::Network(e));
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    if let Some(data) = super::http::parse_sse_data(&line) {
                        if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                            let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            match event_type {
                                "response.output_text.delta" => {
                                    if let Some(delta) = event.get("delta").and_then(|d| d.as_str()) {
                                        yield Ok(TextStreamDelta {
                                            text: delta.to_string(),
                                            event_type: StreamEventType::TextDelta,
                                            finish_reason: None,
                                            usage: None,
                                        });
                                    }
                                }
                                "response.completed" => {
                                    yield Ok(TextStreamDelta {
                                        text: String::new(),
                                        event_type: StreamEventType::Done,
                                        finish_reason: Some(FinishReason::Stop),
                                        usage: None,
                                    });
                                }
                                _ => {} // skip other events
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }
}

// Internal response types for Responses API

#[derive(Deserialize)]
struct ResponsesApiResponse {
    output: Vec<ResponsesOutputItem>,
    status: Option<String>,
    usage: Option<ResponsesUsage>,
}

#[derive(Deserialize)]
struct ResponsesOutputItem {
    r#type: String,
    content: Option<Vec<ResponsesContent>>,
    // function_call fields
    call_id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct ResponsesContent {
    text: Option<String>,
}

#[derive(Deserialize)]
struct ResponsesUsage {
    input_tokens: u32,
    output_tokens: u32,
}
