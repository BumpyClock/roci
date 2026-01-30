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

    fn validate_settings(&self, settings: &GenerationSettings) -> Result<(), RociError> {
        if (settings.temperature.is_some() || settings.top_p.is_some())
            && !self
                .model
                .supports_sampling_params(settings.reasoning_effort)
        {
            return Err(RociError::InvalidArgument(format!(
                "temperature/top_p not supported for model {}",
                self.model.as_str()
            )));
        }
        if settings.text_verbosity.is_some() && !self.model.supports_text_verbosity() {
            return Err(RociError::InvalidArgument(format!(
                "text verbosity not supported for model {}",
                self.model.as_str()
            )));
        }
        Ok(())
    }

    fn build_request_body(&self, request: &ProviderRequest, stream: bool) -> serde_json::Value {
        let input = Self::build_input_items(&request.messages);

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

        let mut text_obj = serde_json::Map::new();
        if let Some(ref fmt) = request.response_format {
            let text_format = match fmt {
                ResponseFormat::JsonObject => Some(serde_json::json!({"type": "json_object"})),
                ResponseFormat::JsonSchema { schema, name } => Some(serde_json::json!({
                    "type": "json_schema",
                    "name": name,
                    "schema": schema,
                    "strict": true,
                })),
                ResponseFormat::Text => None,
            };
            if let Some(format) = text_format {
                text_obj.insert("format".into(), format);
            }
        }
        if let Some(verbosity) = request.settings.text_verbosity {
            text_obj.insert("verbosity".into(), verbosity.to_string().into());
        }
        if !text_obj.is_empty() {
            obj.insert("text".into(), serde_json::Value::Object(text_obj));
        }

        body
    }

    fn build_input_items(messages: &[ModelMessage]) -> Vec<serde_json::Value> {
        let mut input = Vec::new();
        for msg in messages {
            let mut content_parts = Vec::new();
            let mut tool_calls = Vec::new();
            for part in &msg.content {
                match part {
                    ContentPart::Text { text } => {
                        content_parts.push(serde_json::json!({
                            "type": "input_text",
                            "text": text,
                        }));
                    }
                    ContentPart::Image(img) => {
                        let url = format!("data:{};base64,{}", img.mime_type, img.data);
                        content_parts.push(serde_json::json!({
                            "type": "input_image",
                            "image_url": url,
                        }));
                    }
                    ContentPart::ToolCall(tc) => tool_calls.push(tc),
                    ContentPart::ToolResult(_) => {}
                }
            }
            match msg.role {
                Role::System | Role::User | Role::Assistant => {
                    if !content_parts.is_empty() {
                        let content = if content_parts.len() == 1 {
                            if let Some(text) =
                                content_parts[0].get("text").and_then(|v| v.as_str())
                            {
                                serde_json::Value::String(text.to_string())
                            } else {
                                serde_json::Value::Array(content_parts)
                            }
                        } else {
                            serde_json::Value::Array(content_parts)
                        };
                        let role = match msg.role {
                            Role::System => "system",
                            Role::User => "user",
                            Role::Assistant => "assistant",
                            Role::Tool => "tool",
                        };
                        input.push(serde_json::json!({
                            "role": role,
                            "content": content,
                        }));
                    }
                    if matches!(msg.role, Role::Assistant) {
                        for tc in tool_calls {
                            input.push(serde_json::json!({
                                "type": "function_call",
                                "call_id": tc.id,
                                "name": tc.name,
                                "arguments": tc.arguments.to_string(),
                            }));
                        }
                    }
                }
                Role::Tool => {
                    for part in &msg.content {
                        if let ContentPart::ToolResult(tr) = part {
                            input.push(serde_json::json!({
                                "type": "function_call_output",
                                "call_id": tr.tool_call_id,
                                "output": tr.result.to_string(),
                            }));
                        }
                    }
                }
            }
        }
        input
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

    async fn generate_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        self.validate_settings(&request.settings)?;
        let body = self.build_request_body(request, false);
        let url = format!("{}/responses", self.base_url);

        debug!(
            model = self.model.as_str(),
            "OpenAI Responses generate_text"
        );

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
                    tool_calls.push(AgentToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: serde_json::from_str(args)
                            .unwrap_or(serde_json::Value::String(args.clone())),
                        recipient: None,
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
            usage: data
                .usage
                .map(|u| Usage {
                    input_tokens: u.input_tokens,
                    output_tokens: u.output_tokens,
                    total_tokens: u.input_tokens + u.output_tokens,
                    ..Default::default()
                })
                .unwrap_or_default(),
            tool_calls,
            finish_reason,
        })
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        self.validate_settings(&request.settings)?;
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
            let mut call_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
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
                                "response.output_item.added" => {
                                    if let Some(item) = event.get("item") {
                                        if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                                            if let (Some(id), Some(name)) = (
                                                item.get("call_id").and_then(|v| v.as_str()).or_else(|| item.get("id").and_then(|v| v.as_str())),
                                                item.get("name").and_then(|v| v.as_str()),
                                            ) {
                                                call_names.insert(id.to_string(), name.to_string());
                                            }
                                        }
                                    }
                                }
                                "response.function_call_arguments.done" => {
                                    let call_id = event.get("call_id")
                                        .and_then(|v| v.as_str())
                                        .or_else(|| event.get("item_id").and_then(|v| v.as_str()))
                                        .map(|v| v.to_string());
                                    let name = event.get("name")
                                        .and_then(|v| v.as_str())
                                        .map(|v| v.to_string())
                                        .or_else(|| call_id.as_ref().and_then(|id| call_names.get(id).cloned()));
                                    if let (Some(id), Some(name), Some(args)) = (
                                        call_id,
                                        name,
                                        event.get("arguments").and_then(|v| v.as_str()),
                                    ) {
                                        let arguments = serde_json::from_str(args)
                                            .unwrap_or(serde_json::Value::String(args.to_string()));
                                        yield Ok(TextStreamDelta {
                                            text: String::new(),
                                            event_type: StreamEventType::ToolCallDelta,
                                            tool_call: Some(AgentToolCall { id, name, arguments, recipient: None }),
                                            finish_reason: None,
                                            usage: None,
                                        });
                                    }
                                }
                                "response.output_text.delta" => {
                                    if let Some(delta) = event.get("delta").and_then(|d| d.as_str()) {
                                        yield Ok(TextStreamDelta {
                                            text: delta.to_string(),
                                            event_type: StreamEventType::TextDelta,
                                            tool_call: None,
                                            finish_reason: None,
                                            usage: None,
                                        });
                                    }
                                }
                                "response.completed" => {
                                    let finish = event.get("response")
                                        .and_then(|r| r.get("status"))
                                        .and_then(|v| v.as_str())
                                        .and_then(|status| match status {
                                            "completed" => Some(FinishReason::Stop),
                                            "incomplete" => Some(FinishReason::Length),
                                            _ => None,
                                        });
                                    let usage = event.get("response")
                                        .and_then(|r| r.get("usage"))
                                        .and_then(|u| {
                                            Some(Usage {
                                                input_tokens: u.get("input_tokens")?.as_u64()? as u32,
                                                output_tokens: u.get("output_tokens")?.as_u64()? as u32,
                                                total_tokens: u.get("total_tokens")?.as_u64()? as u32,
                                                ..Default::default()
                                            })
                                        });
                                    yield Ok(TextStreamDelta {
                                        text: String::new(),
                                        event_type: StreamEventType::Done,
                                        tool_call: None,
                                        finish_reason: finish.or(Some(FinishReason::Stop)),
                                        usage,
                                    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt5_rejects_sampling_settings() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None);
        let settings = GenerationSettings {
            temperature: Some(0.7),
            ..Default::default()
        };
        let err = provider.validate_settings(&settings).unwrap_err();
        assert!(matches!(err, RociError::InvalidArgument(_)));
    }

    #[test]
    fn gpt52_rejects_sampling_without_reasoning_none() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt52, "test-key".to_string(), None);
        let settings = GenerationSettings {
            temperature: Some(0.7),
            ..Default::default()
        };
        let err = provider.validate_settings(&settings).unwrap_err();
        assert!(matches!(err, RociError::InvalidArgument(_)));
    }

    #[test]
    fn gpt52_allows_sampling_with_reasoning_none() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt52, "test-key".to_string(), None);
        let settings = GenerationSettings {
            temperature: Some(0.7),
            reasoning_effort: Some(ReasoningEffort::None),
            ..Default::default()
        };
        assert!(provider.validate_settings(&settings).is_ok());
    }

    #[test]
    fn gpt41_allows_sampling_settings() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt41Nano, "test-key".to_string(), None);
        let settings = GenerationSettings {
            temperature: Some(0.7),
            top_p: Some(0.9),
            ..Default::default()
        };
        assert!(provider.validate_settings(&settings).is_ok());
    }

    #[test]
    fn gpt41_rejects_text_verbosity_setting() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt41Nano, "test-key".to_string(), None);
        let settings = GenerationSettings {
            text_verbosity: Some(TextVerbosity::Low),
            ..Default::default()
        };
        let err = provider.validate_settings(&settings).unwrap_err();
        assert!(matches!(err, RociError::InvalidArgument(_)));
    }

    #[test]
    fn gpt5_allows_text_verbosity_setting() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None);
        let settings = GenerationSettings {
            text_verbosity: Some(TextVerbosity::High),
            ..Default::default()
        };
        assert!(provider.validate_settings(&settings).is_ok());
    }

    #[test]
    fn request_body_includes_text_verbosity_and_format() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None);
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: GenerationSettings {
                text_verbosity: Some(TextVerbosity::Low),
                ..Default::default()
            },
            tools: None,
            response_format: Some(ResponseFormat::JsonObject),
        };
        let body = provider.build_request_body(&request, false);
        assert_eq!(body["text"]["verbosity"], "low");
        assert_eq!(body["text"]["format"]["type"], "json_object");
    }
}
