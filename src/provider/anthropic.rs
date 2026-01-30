//! Anthropic Messages API provider.

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use serde::Deserialize;
use tracing::debug;

use crate::error::RociError;
use crate::models::anthropic::AnthropicModel;
use crate::models::capabilities::ModelCapabilities;
use crate::types::*;

use super::http::{anthropic_headers, shared_client};
use super::{ModelProvider, ProviderRequest, ProviderResponse};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
const API_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    model: AnthropicModel,
    api_key: String,
    base_url: String,
    capabilities: ModelCapabilities,
}

impl AnthropicProvider {
    pub fn new(model: AnthropicModel, api_key: String, base_url: Option<String>) -> Self {
        let capabilities = model.capabilities();
        Self {
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            model,
            api_key,
            capabilities,
        }
    }

    fn build_request_body(&self, request: &ProviderRequest, stream: bool) -> serde_json::Value {
        let mut system_parts = Vec::new();
        let mut messages = Vec::new();

        for msg in &request.messages {
            match msg.role {
                Role::System => {
                    system_parts.push(msg.text());
                }
                Role::User => {
                    let content = build_anthropic_content(&msg.content);
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": content,
                    }));
                }
                Role::Assistant => {
                    let text = msg.text();
                    let tool_calls = msg.tool_calls();
                    if tool_calls.is_empty() {
                        messages.push(serde_json::json!({
                            "role": "assistant",
                            "content": text,
                        }));
                    } else {
                        let mut content: Vec<serde_json::Value> = Vec::new();
                        if !text.is_empty() {
                            content.push(serde_json::json!({"type": "text", "text": text}));
                        }
                        for tc in tool_calls {
                            content.push(serde_json::json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": tc.arguments,
                            }));
                        }
                        messages.push(serde_json::json!({
                            "role": "assistant",
                            "content": content,
                        }));
                    }
                }
                Role::Tool => {
                    for part in &msg.content {
                        if let ContentPart::ToolResult(tr) = part {
                            messages.push(serde_json::json!({
                                "role": "user",
                                "content": [{
                                    "type": "tool_result",
                                    "tool_use_id": tr.tool_call_id,
                                    "content": tr.result.to_string(),
                                    "is_error": tr.is_error,
                                }],
                            }));
                        }
                    }
                }
            }
        }

        let max_tokens = request.settings.max_tokens.unwrap_or(4096);

        let mut body = serde_json::json!({
            "model": self.model.as_str(),
            "messages": messages,
            "max_tokens": max_tokens,
            "stream": stream,
        });

        let obj = body.as_object_mut().unwrap();

        if !system_parts.is_empty() {
            obj.insert("system".into(), system_parts.join("\n").into());
        }
        if let Some(temp) = request.settings.temperature {
            obj.insert("temperature".into(), temp.into());
        }
        if let Some(top_p) = request.settings.top_p {
            obj.insert("top_p".into(), top_p.into());
        }
        if let Some(ref stops) = request.settings.stop_sequences {
            obj.insert("stop_sequences".into(), serde_json::json!(stops));
        }

        if let Some(ref tools) = request.tools {
            if !tools.is_empty() {
                let tool_defs: Vec<serde_json::Value> = tools
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "name": t.name,
                            "description": t.description,
                            "input_schema": t.parameters,
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
impl ModelProvider for AnthropicProvider {
    fn provider_name(&self) -> &str {
        "anthropic"
    }

    fn model_id(&self) -> &str {
        self.model.as_str()
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        let body = self.build_request_body(request, false);
        let url = format!("{}/messages", self.base_url);

        debug!(model = self.model.as_str(), "Anthropic generate_text");

        let resp = shared_client()
            .post(&url)
            .headers(anthropic_headers(&self.api_key, API_VERSION))
            .json(&body)
            .send()
            .await?;

        let status = resp.status().as_u16();
        if status != 200 {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(super::http::status_to_error(status, &body_text));
        }

        let data: AnthropicResponse = resp.json().await?;

        let mut text = String::new();
        let mut tool_calls = Vec::new();

        for block in &data.content {
            match block.r#type.as_str() {
                "text" => {
                    if let Some(ref t) = block.text {
                        text.push_str(t);
                    }
                }
                "tool_use" => {
                    if let (Some(ref id), Some(ref name), Some(ref input)) =
                        (&block.id, &block.name, &block.input)
                    {
                        tool_calls.push(message::AgentToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            arguments: input.clone(),
                            recipient: None,
                        });
                    }
                }
                _ => {}
            }
        }

        let finish_reason = match data.stop_reason.as_deref() {
            Some("end_turn") => Some(FinishReason::Stop),
            Some("max_tokens") => Some(FinishReason::Length),
            Some("tool_use") => Some(FinishReason::ToolCalls),
            _ => None,
        };

        Ok(ProviderResponse {
            text,
            usage: Usage {
                input_tokens: data.usage.input_tokens,
                output_tokens: data.usage.output_tokens,
                total_tokens: data.usage.input_tokens + data.usage.output_tokens,
                cache_read_tokens: data.usage.cache_read_input_tokens,
                cache_creation_tokens: data.usage.cache_creation_input_tokens,
                ..Default::default()
            },
            tool_calls,
            finish_reason,
        })
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        let body = self.build_request_body(request, true);
        let url = format!("{}/messages", self.base_url);

        debug!(model = self.model.as_str(), "Anthropic stream_text");

        let resp = shared_client()
            .post(&url)
            .headers(anthropic_headers(&self.api_key, API_VERSION))
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
                            let event_type_str = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            match event_type_str {
                                "content_block_delta" => {
                                    if let Some(delta) = event.get("delta") {
                                        if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                                            yield Ok(TextStreamDelta {
                                                text: text.to_string(),
                                                event_type: StreamEventType::TextDelta,
                                                tool_call: None,
                                                finish_reason: None,
                                                usage: None,
                                            });
                                        }
                                    }
                                }
                                "message_stop" => {
                                    yield Ok(TextStreamDelta {
                                        text: String::new(),
                                        event_type: StreamEventType::Done,
                                        tool_call: None,
                                        finish_reason: Some(FinishReason::Stop),
                                        usage: None,
                                    });
                                }
                                "message_delta" => {
                                    let stop = event.get("delta")
                                        .and_then(|d| d.get("stop_reason"))
                                        .and_then(|s| s.as_str());
                                    let finish = match stop {
                                        Some("end_turn") => Some(FinishReason::Stop),
                                        Some("max_tokens") => Some(FinishReason::Length),
                                        Some("tool_use") => Some(FinishReason::ToolCalls),
                                        _ => None,
                                    };
                                    if finish.is_some() {
                                        let usage = event.get("usage").and_then(|u| {
                                            Some(Usage {
                                                output_tokens: u.get("output_tokens")?.as_u64()? as u32,
                                                ..Default::default()
                                            })
                                        });
                                        yield Ok(TextStreamDelta {
                                            text: String::new(),
                                            event_type: StreamEventType::Done,
                                            tool_call: None,
                                            finish_reason: finish,
                                            usage,
                                        });
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }
}

fn build_anthropic_content(parts: &[ContentPart]) -> serde_json::Value {
    if parts.len() == 1 {
        if let ContentPart::Text { ref text } = parts[0] {
            return serde_json::Value::String(text.clone());
        }
    }

    let content: Vec<serde_json::Value> = parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(serde_json::json!({
                "type": "text",
                "text": text,
            })),
            ContentPart::Image(img) => Some(serde_json::json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": img.mime_type,
                    "data": img.data,
                }
            })),
            _ => None,
        })
        .collect();

    serde_json::json!(content)
}

// Internal Anthropic response types

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Deserialize)]
struct AnthropicContentBlock {
    r#type: String,
    text: Option<String>,
    id: Option<String>,
    name: Option<String>,
    input: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
}
