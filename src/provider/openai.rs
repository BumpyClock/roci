//! OpenAI Chat Completions API provider.

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

pub struct OpenAiProvider {
    model: OpenAiModel,
    api_key: String,
    base_url: String,
    capabilities: ModelCapabilities,
}

impl OpenAiProvider {
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
        let messages = request
            .messages
            .iter()
            .map(|m| message_to_openai(m))
            .collect::<Vec<_>>();

        let mut body = serde_json::json!({
            "model": self.model.as_str(),
            "messages": messages,
            "stream": stream,
        });

        let obj = body.as_object_mut().unwrap();

        if let Some(max) = request.settings.max_tokens {
            obj.insert("max_tokens".into(), max.into());
        }
        if let Some(temp) = request.settings.temperature {
            obj.insert("temperature".into(), temp.into());
        }
        if let Some(top_p) = request.settings.top_p {
            obj.insert("top_p".into(), top_p.into());
        }
        if let Some(ref stops) = request.settings.stop_sequences {
            obj.insert("stop".into(), serde_json::json!(stops));
        }
        if let Some(pp) = request.settings.presence_penalty {
            obj.insert("presence_penalty".into(), pp.into());
        }
        if let Some(fp) = request.settings.frequency_penalty {
            obj.insert("frequency_penalty".into(), fp.into());
        }
        if let Some(seed) = request.settings.seed {
            obj.insert("seed".into(), seed.into());
        }
        if let Some(ref user) = request.settings.user {
            obj.insert("user".into(), user.clone().into());
        }

        if let Some(ref tools) = request.tools {
            if !tools.is_empty() {
                let tool_defs: Vec<serde_json::Value> = tools
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "type": "function",
                            "function": {
                                "name": t.name,
                                "description": t.description,
                                "parameters": t.parameters,
                            }
                        })
                    })
                    .collect();
                obj.insert("tools".into(), tool_defs.into());
            }
        }

        if let Some(ref fmt) = request.response_format {
            match fmt {
                ResponseFormat::JsonObject => {
                    obj.insert("response_format".into(), serde_json::json!({"type": "json_object"}));
                }
                ResponseFormat::JsonSchema { schema, name } => {
                    obj.insert("response_format".into(), serde_json::json!({
                        "type": "json_schema",
                        "json_schema": {
                            "name": name,
                            "schema": schema,
                            "strict": true,
                        }
                    }));
                }
                _ => {}
            }
        }

        body
    }
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn model_id(&self) -> &str {
        self.model.as_str()
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(&self, request: &ProviderRequest) -> Result<ProviderResponse, RociError> {
        let body = self.build_request_body(request, false);
        let url = format!("{}/chat/completions", self.base_url);

        debug!(model = self.model.as_str(), "OpenAI generate_text");

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

        let data: OpenAiChatResponse = resp.json().await?;
        let choice = data.choices.into_iter().next().ok_or_else(|| {
            RociError::api(200, "No choices in OpenAI response")
        })?;

        let tool_calls = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| message::AgentToolCall {
                id: tc.id,
                name: tc.function.name,
                arguments: serde_json::from_str(&tc.function.arguments)
                    .unwrap_or(serde_json::Value::String(tc.function.arguments)),
            })
            .collect();

        let finish_reason = choice.finish_reason.as_deref().and_then(parse_finish_reason);

        Ok(ProviderResponse {
            text: choice.message.content.unwrap_or_default(),
            usage: data.usage.map(|u| Usage {
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
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
        let url = format!("{}/chat/completions", self.base_url);

        debug!(model = self.model.as_str(), "OpenAI stream_text");

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
                        match serde_json::from_str::<OpenAiStreamChunk>(data) {
                            Ok(chunk) => {
                                if let Some(choice) = chunk.choices.into_iter().next() {
                                    let text = choice.delta.content.unwrap_or_default();
                                    let finish = choice.finish_reason.as_deref().and_then(parse_finish_reason);
                                    let event_type = if finish.is_some() {
                                        StreamEventType::Done
                                    } else {
                                        StreamEventType::TextDelta
                                    };
                                    yield Ok(TextStreamDelta {
                                        text,
                                        event_type,
                                        finish_reason: finish,
                                        usage: chunk.usage.map(|u| Usage {
                                            input_tokens: u.prompt_tokens,
                                            output_tokens: u.completion_tokens,
                                            total_tokens: u.total_tokens,
                                            ..Default::default()
                                        }),
                                    });
                                }
                            }
                            Err(_) => {} // skip unparseable chunks
                        }
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }
}

fn parse_finish_reason(s: &str) -> Option<FinishReason> {
    match s {
        "stop" => Some(FinishReason::Stop),
        "length" => Some(FinishReason::Length),
        "tool_calls" => Some(FinishReason::ToolCalls),
        "content_filter" => Some(FinishReason::ContentFilter),
        _ => None,
    }
}

fn message_to_openai(msg: &ModelMessage) -> serde_json::Value {
    let role = match msg.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };

    // Simple single-text message
    if msg.content.len() == 1 {
        if let ContentPart::Text { ref text } = msg.content[0] {
            return serde_json::json!({ "role": role, "content": text });
        }
        if let ContentPart::ToolResult(ref tr) = msg.content[0] {
            return serde_json::json!({
                "role": "tool",
                "tool_call_id": tr.tool_call_id,
                "content": tr.result.to_string(),
            });
        }
    }

    // Multi-part content
    let parts: Vec<serde_json::Value> = msg
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(serde_json::json!({
                "type": "text",
                "text": text,
            })),
            ContentPart::Image(img) => Some(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{};base64,{}", img.mime_type, img.data) }
            })),
            ContentPart::ToolCall(tc) => Some(serde_json::json!({
                "type": "function",
                "id": tc.id,
                "function": {
                    "name": tc.name,
                    "arguments": tc.arguments.to_string(),
                }
            })),
            ContentPart::ToolResult(_) => None, // handled at message level
        })
        .collect();

    // Check if assistant with tool calls
    let tool_calls: Vec<&message::AgentToolCall> = msg.tool_calls();
    if !tool_calls.is_empty() {
        let tc_json: Vec<serde_json::Value> = tool_calls
            .iter()
            .map(|tc| {
                serde_json::json!({
                    "id": tc.id,
                    "type": "function",
                    "function": {
                        "name": tc.name,
                        "arguments": tc.arguments.to_string(),
                    }
                })
            })
            .collect();
        let text = msg.text();
        return serde_json::json!({
            "role": role,
            "content": if text.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(text) },
            "tool_calls": tc_json,
        });
    }

    serde_json::json!({ "role": role, "content": parts })
}

// OpenAI API response types (internal)

#[derive(Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Deserialize)]
struct OpenAiToolCall {
    id: String,
    function: OpenAiFunction,
}

#[derive(Deserialize)]
struct OpenAiFunction {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Deserialize)]
struct OpenAiStreamChunk {
    choices: Vec<OpenAiStreamChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiStreamChoice {
    delta: OpenAiStreamDelta,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiStreamDelta {
    content: Option<String>,
}
