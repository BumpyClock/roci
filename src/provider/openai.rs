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

use super::format::tool_result_to_string;
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
            .map(message_to_openai)
            .collect::<Vec<_>>();

        let mut body = serde_json::json!({
            "model": self.model.as_str(),
            "messages": messages,
            "stream": stream,
        });

        let obj = body.as_object_mut().unwrap();

        let is_gpt5 = self.model.is_gpt5_family_id();
        if let Some(max) = request.settings.max_tokens {
            let key = if is_gpt5 {
                "max_completion_tokens"
            } else {
                "max_tokens"
            };
            obj.insert(key.into(), max.into());
        }
        if let Some(temp) = request.settings.temperature {
            if !is_gpt5 {
                obj.insert("temperature".into(), temp.into());
            }
        }
        if let Some(top_p) = request.settings.top_p {
            if !is_gpt5 {
                obj.insert("top_p".into(), top_p.into());
            }
        }
        if let Some(ref stops) = request.settings.stop_sequences {
            obj.insert("stop".into(), serde_json::json!(stops));
        }
        if let Some(pp) = request.settings.presence_penalty {
            if !is_gpt5 {
                obj.insert("presence_penalty".into(), pp.into());
            }
        }
        if let Some(fp) = request.settings.frequency_penalty {
            if !is_gpt5 {
                obj.insert("frequency_penalty".into(), fp.into());
            }
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
                    obj.insert(
                        "response_format".into(),
                        serde_json::json!({"type": "json_object"}),
                    );
                }
                ResponseFormat::JsonSchema { schema, name } => {
                    obj.insert(
                        "response_format".into(),
                        serde_json::json!({
                            "type": "json_schema",
                            "json_schema": {
                                "name": name,
                                "schema": schema,
                                "strict": true,
                            }
                        }),
                    );
                }
                _ => {}
            }
        }

        body
    }
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn provider_name(&self) -> &str {
        "openai"
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
        let choice = data
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| RociError::api(200, "No choices in OpenAI response"))?;

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
                recipient: None,
            })
            .collect();

        let finish_reason = choice
            .finish_reason
            .as_deref()
            .and_then(parse_finish_reason);

        Ok(ProviderResponse {
            text: choice.message.content.unwrap_or_default(),
            usage: data
                .usage
                .map(|u| Usage {
                    input_tokens: u.prompt_tokens,
                    output_tokens: u.completion_tokens,
                    total_tokens: u.total_tokens,
                    ..Default::default()
                })
                .unwrap_or_default(),
            tool_calls,
            finish_reason,
            thinking: Vec::new(),
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

        struct ToolCallBuilder {
            id: Option<String>,
            name: Option<String>,
            arguments: String,
        }
        let stream = async_stream::stream! {
            let mut buffer = String::new();
            let mut tool_calls: std::collections::HashMap<usize, ToolCallBuilder> = std::collections::HashMap::new();
            let mut chunk_count: u64 = 0;
            let mut line_count: u64 = 0;
            let mut byte_count: u64 = 0;
            futures::pin_mut!(byte_stream);

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        yield Err(RociError::Network(e));
                        break;
                    }
                };

                chunk_count += 1;
                byte_count += chunk.len() as u64;
                if debug_enabled() && chunk_count == 1 {
                    debug!(chunk_len = chunk.len(), "OpenAI stream first chunk");
                }

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    line_count += 1;
                    if line == "data: [DONE]" {
                        if debug_enabled() {
                            debug!(chunk_count, line_count, byte_count, "OpenAI stream done");
                        }
                        yield Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::Done,
                            tool_call: None,
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        });
                        continue;
                    }

                    if let Some(data) = super::http::parse_sse_data(&line) {
                        if let Ok(chunk) = serde_json::from_str::<OpenAiStreamChunk>(data) {
                            if let Some(choice) = chunk.choices.into_iter().next() {
                                if let Some(deltas) = choice.delta.tool_calls {
                                    for delta in deltas {
                                        let entry = tool_calls.entry(delta.index).or_insert_with(|| ToolCallBuilder {
                                            id: None,
                                            name: None,
                                            arguments: String::new(),
                                        });
                                        if let Some(id) = delta.id {
                                            entry.id = Some(id);
                                        }
                                        if let Some(func) = delta.function {
                                            if let Some(name) = func.name {
                                                entry.name = Some(name);
                                            }
                                            if let Some(args) = func.arguments {
                                                entry.arguments.push_str(&args);
                                            }
                                        }
                                    }
                                }
                                if let Some(text) = choice.delta.content {
                                    yield Ok(TextStreamDelta {
                                        text,
                                        event_type: StreamEventType::TextDelta,
                                        tool_call: None,
                                        finish_reason: None,
                                        usage: None,
                                        reasoning: None,
                                        reasoning_signature: None,
                                        reasoning_type: None,
                                    });
                                }
                                let finish = choice.finish_reason.as_deref().and_then(parse_finish_reason);
                                if let Some(reason) = finish {
                                    if reason == FinishReason::ToolCalls {
                                        let mut indices = tool_calls.keys().copied().collect::<Vec<_>>();
                                        indices.sort_unstable();
                                        for index in indices {
                                            if let Some(builder) = tool_calls.remove(&index) {
                                                if let (Some(id), Some(name)) = (builder.id, builder.name) {
                                                    let args = serde_json::from_str(&builder.arguments)
                                                        .unwrap_or(serde_json::Value::String(builder.arguments));
                                                    yield Ok(TextStreamDelta {
                                                        text: String::new(),
                                                        event_type: StreamEventType::ToolCallDelta,
                                                        tool_call: Some(AgentToolCall { id, name, arguments: args, recipient: None }),
                                                        finish_reason: None,
                                                        usage: None,
                                                        reasoning: None,
                                                        reasoning_signature: None,
                                                        reasoning_type: None,
                                                    });
                                                }
                                            }
                                        }
                                    }
                                    yield Ok(TextStreamDelta {
                                        text: String::new(),
                                        event_type: StreamEventType::Done,
                                        tool_call: None,
                                        finish_reason: Some(reason),
                                        usage: chunk.usage.map(|u| Usage {
                                            input_tokens: u.prompt_tokens,
                                            output_tokens: u.completion_tokens,
                                            total_tokens: u.total_tokens,
                                            ..Default::default()
                                        }),
                                        reasoning: None,
                                        reasoning_signature: None,
                                        reasoning_type: None,
                                    });
                                }
                            }
                        } else if debug_enabled() {
                            debug!(line_len = line.len(), "OpenAI stream parse failed");
                        }
                    }
                }
            }

            if debug_enabled() {
                debug!(chunk_count, line_count, byte_count, "OpenAI stream ended");
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

fn debug_enabled() -> bool {
    matches!(std::env::var("HOMIE_DEBUG").as_deref(), Ok("1"))
        || matches!(std::env::var("HOME_DEBUG").as_deref(), Ok("1"))
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
                "content": tool_result_to_string(&tr.result),
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
            ContentPart::Thinking(_) => None,
            ContentPart::RedactedThinking(_) => None,
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
    tool_calls: Option<Vec<OpenAiStreamToolCallDelta>>,
}

#[derive(Deserialize)]
struct OpenAiStreamToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<OpenAiStreamFunctionDelta>,
}

#[derive(Deserialize)]
struct OpenAiStreamFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(
        max_tokens: Option<u32>,
        temperature: Option<f64>,
        top_p: Option<f64>,
        presence_penalty: Option<f64>,
        frequency_penalty: Option<f64>,
    ) -> GenerationSettings {
        GenerationSettings {
            max_tokens,
            temperature,
            top_p,
            top_k: None,
            stop_sequences: None,
            presence_penalty,
            frequency_penalty,
            seed: None,
            reasoning_effort: None,
            text_verbosity: None,
            response_format: None,
            openai_responses: None,
            user: None,
            anthropic: None,
            google: None,
            tool_choice: None,
        }
    }

    #[test]
    fn chat_request_uses_max_completion_tokens_for_gpt5() {
        let provider = OpenAiProvider::new(
            OpenAiModel::Custom("gpt-5-nano".to_string()),
            "test-key".to_string(),
            None,
        );
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(Some(128), Some(0.4), Some(0.5), Some(0.1), Some(0.2)),
            tools: None,
            response_format: None,
        };

        let body = provider.build_request_body(&request, false);

        assert_eq!(
            body.get("max_completion_tokens").and_then(|v| v.as_u64()),
            Some(128)
        );
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("temperature").is_none());
        assert!(body.get("top_p").is_none());
        assert!(body.get("presence_penalty").is_none());
        assert!(body.get("frequency_penalty").is_none());
    }

    #[test]
    fn tool_result_uses_plain_string_content() {
        let provider = OpenAiProvider::new(OpenAiModel::Gpt4o, "test-key".to_string(), None);
        let message =
            ModelMessage::tool_result("call_1", serde_json::Value::String("ok".to_string()), false);
        let request = ProviderRequest {
            messages: vec![message],
            settings: settings(None, None, None, None, None),
            tools: None,
            response_format: None,
        };

        let body = provider.build_request_body(&request, false);
        assert_eq!(body["messages"][0]["content"], "ok");
    }
}
