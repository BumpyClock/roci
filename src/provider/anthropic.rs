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

/// Beta feature flags for interleaved thinking + fine-grained tool streaming.
const BETA_FLAGS: &str = "interleaved-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14";

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

    /// Check if thinking mode is enabled in the request settings.
    fn thinking_enabled(request: &ProviderRequest) -> bool {
        request
            .settings
            .anthropic
            .as_ref()
            .and_then(|a| a.thinking.as_ref())
            .is_some_and(|t| matches!(t, ThinkingMode::Enabled { .. }))
    }

    /// Build HTTP headers, always including beta flags.
    fn build_headers(&self) -> reqwest::header::HeaderMap {
        anthropic_headers(&self.api_key, API_VERSION, Some(BETA_FLAGS))
    }

    fn build_request_body(&self, request: &ProviderRequest, stream: bool) -> serde_json::Value {
        let thinking = Self::thinking_enabled(request);
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
                    let mut content: Vec<serde_json::Value> = Vec::new();

                    // Include thinking blocks when thinking is enabled
                    for part in &msg.content {
                        match part {
                            ContentPart::Thinking(tc) => {
                                if thinking {
                                    content.push(serde_json::json!({
                                        "type": "thinking",
                                        "thinking": tc.thinking,
                                        "signature": tc.signature,
                                    }));
                                }
                            }
                            ContentPart::RedactedThinking(rc) => {
                                if thinking {
                                    content.push(serde_json::json!({
                                        "type": "redacted_thinking",
                                        "data": rc.data,
                                        "signature": rc.signature,
                                    }));
                                }
                            }
                            ContentPart::Text { text } => {
                                if !text.is_empty() {
                                    content.push(serde_json::json!({"type": "text", "text": text}));
                                }
                            }
                            ContentPart::ToolCall(tc) => {
                                content.push(serde_json::json!({
                                    "type": "tool_use",
                                    "id": tc.id,
                                    "name": tc.name,
                                    "input": tc.arguments,
                                }));
                            }
                            _ => {}
                        }
                    }

                    if content.is_empty() {
                        let text = msg.text();
                        if !text.is_empty() {
                            messages.push(serde_json::json!({
                                "role": "assistant",
                                "content": text,
                            }));
                        }
                    } else {
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

        let max_tokens = if thinking {
            // When thinking is enabled, ensure sufficient output budget.
            let budget = request
                .settings
                .anthropic
                .as_ref()
                .and_then(|a| a.thinking.as_ref())
                .and_then(|t| match t {
                    ThinkingMode::Enabled { budget_tokens } => Some(*budget_tokens),
                    _ => None,
                })
                .unwrap_or(0);
            let min = std::cmp::max(16_384, budget + 4096);
            std::cmp::max(request.settings.max_tokens.unwrap_or(min), min)
        } else {
            request.settings.max_tokens.unwrap_or(4096)
        };

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
            // Note: temperature is not allowed when thinking is enabled
            if !thinking {
                obj.insert("temperature".into(), temp.into());
            }
        }
        if let Some(top_p) = request.settings.top_p {
            obj.insert("top_p".into(), top_p.into());
        }
        if let Some(top_k) = request.settings.top_k {
            obj.insert("top_k".into(), top_k.into());
        }
        if let Some(ref stops) = request.settings.stop_sequences {
            obj.insert("stop_sequences".into(), serde_json::json!(stops));
        }

        // Extended thinking
        if let Some(ref anthropic_opts) = request.settings.anthropic {
            if let Some(ref thinking_mode) = anthropic_opts.thinking {
                match thinking_mode {
                    ThinkingMode::Enabled { budget_tokens } => {
                        obj.insert(
                            "thinking".into(),
                            serde_json::json!({
                                "type": "enabled",
                                "budget_tokens": budget_tokens,
                            }),
                        );
                    }
                    ThinkingMode::Disabled => {
                        // Explicitly disabled — don't send thinking field
                    }
                }
            }
            if let Some(ref metadata) = anthropic_opts.metadata {
                obj.insert("metadata".into(), serde_json::json!(metadata));
            }
        }

        // Tool choice
        if let Some(ref tool_choice) = request.settings.tool_choice {
            match tool_choice {
                ToolChoice::Auto => {
                    obj.insert("tool_choice".into(), serde_json::json!({"type": "auto"}));
                }
                ToolChoice::Required => {
                    obj.insert("tool_choice".into(), serde_json::json!({"type": "any"}));
                }
                ToolChoice::None => {
                    // Anthropic doesn't have "none" — just don't send tools
                }
                ToolChoice::Function(name) => {
                    obj.insert(
                        "tool_choice".into(),
                        serde_json::json!({"type": "tool", "name": name}),
                    );
                }
            }
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
            .headers(self.build_headers())
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
        let mut thinking_blocks = Vec::new();

        for block in &data.content {
            match block.r#type.as_str() {
                "text" => {
                    if let Some(ref t) = block.text {
                        text.push_str(t);
                    }
                }
                "thinking" => {
                    if let (Some(ref thinking), Some(ref signature)) =
                        (&block.thinking, &block.signature)
                    {
                        thinking_blocks.push(ContentPart::Thinking(ThinkingContent {
                            thinking: thinking.clone(),
                            signature: signature.clone(),
                        }));
                    }
                }
                "redacted_thinking" => {
                    if let Some(ref signature) = block.signature {
                        thinking_blocks.push(ContentPart::RedactedThinking(
                            RedactedThinkingContent {
                                data: block.data.clone().unwrap_or_default(),
                                signature: signature.clone(),
                            },
                        ));
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
            thinking: thinking_blocks,
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
            .headers(self.build_headers())
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
            let mut current_block_type: Option<String> = None;
            let mut current_tool_id: Option<String> = None;
            let mut current_tool_name: Option<String> = None;
            let mut current_tool_input = String::new();
            let mut saw_tool_use = false;
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
                                "content_block_start" => {
                                    if let Some(block) = event.get("content_block") {
                                        let btype = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                        current_block_type = Some(btype.to_string());
                                        match btype {
                                            "tool_use" => {
                                                current_tool_id = block.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
                                                current_tool_name = block.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());
                                                current_tool_input.clear();
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                "content_block_delta" => {
                                    if let Some(delta) = event.get("delta") {
                                        let delta_type = delta.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                        match delta_type {
                                            "text_delta" => {
                                                if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                                                    yield Ok(TextStreamDelta {
                                                        text: text.to_string(),
                                                        event_type: StreamEventType::TextDelta,
                                                        tool_call: None,
                                                        finish_reason: None,
                                                        usage: None,
                                                        reasoning: None,
                                                        reasoning_signature: None,
                                                        reasoning_type: None,
                                                    });
                                                }
                                            }
                                            "thinking_delta" => {
                                                if let Some(thinking) = delta.get("thinking").and_then(|t| t.as_str()) {
                                                    yield Ok(TextStreamDelta {
                                                        text: String::new(),
                                                        event_type: StreamEventType::Reasoning,
                                                        tool_call: None,
                                                        finish_reason: None,
                                                        usage: None,
                                                        reasoning: Some(thinking.to_string()),
                                                        reasoning_signature: None,
                                                        reasoning_type: current_block_type.clone(),
                                                    });
                                                }
                                            }
                                            "signature_delta" => {
                                                if let Some(sig) = delta.get("signature").and_then(|t| t.as_str()) {
                                                    yield Ok(TextStreamDelta {
                                                        text: String::new(),
                                                        event_type: StreamEventType::Reasoning,
                                                        tool_call: None,
                                                        finish_reason: None,
                                                        usage: None,
                                                        reasoning: None,
                                                        reasoning_signature: Some(sig.to_string()),
                                                        reasoning_type: current_block_type.clone(),
                                                    });
                                                }
                                            }
                                            "input_json_delta" => {
                                                if let Some(json) = delta.get("partial_json").and_then(|t| t.as_str()) {
                                                    current_tool_input.push_str(json);
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                "content_block_stop" => {
                                    if current_block_type.as_deref() == Some("tool_use") {
                                        if let (Some(id), Some(name)) = (current_tool_id.take(), current_tool_name.take()) {
                                            let args = serde_json::from_str(&current_tool_input)
                                                .unwrap_or(serde_json::Value::String(current_tool_input.clone()));
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
                                            saw_tool_use = true;
                                            current_tool_input.clear();
                                        }
                                    }
                                    current_block_type = None;
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
                                            finish_reason: if saw_tool_use { Some(FinishReason::ToolCalls) } else { finish },
                                            usage,
                                            reasoning: None,
                                            reasoning_signature: None,
                                            reasoning_type: None,
                                        });
                                    }
                                }
                                "message_stop" => {
                                    yield Ok(TextStreamDelta {
                                        text: String::new(),
                                        event_type: StreamEventType::Done,
                                        tool_call: None,
                                        finish_reason: if saw_tool_use { Some(FinishReason::ToolCalls) } else { Some(FinishReason::Stop) },
                                        usage: None,
                                        reasoning: None,
                                        reasoning_signature: None,
                                        reasoning_type: None,
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
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<serde_json::Value>,
    /// Thinking content (for type="thinking").
    #[serde(default)]
    thinking: Option<String>,
    /// Redacted thinking data (for type="redacted_thinking").
    #[serde(default)]
    data: Option<String>,
    /// Signature for thinking/redacted_thinking blocks.
    #[serde(default)]
    signature: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ToolDefinition;

    fn settings() -> GenerationSettings {
        GenerationSettings::default()
    }

    #[test]
    fn request_body_includes_thinking_config() {
        let provider =
            AnthropicProvider::new(AnthropicModel::ClaudeSonnet4, "test-key".to_string(), None);
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: GenerationSettings {
                anthropic: Some(AnthropicOptions {
                    thinking: Some(ThinkingMode::Enabled {
                        budget_tokens: 10000,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            tools: None,
            response_format: None,
            session_id: None,
            transport: None,
        };
        let body = provider.build_request_body(&request, false);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 10000);
        // temperature should not be present when thinking is enabled
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn request_body_omits_thinking_when_disabled() {
        let provider =
            AnthropicProvider::new(AnthropicModel::ClaudeSonnet4, "test-key".to_string(), None);
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: GenerationSettings {
                anthropic: Some(AnthropicOptions {
                    thinking: Some(ThinkingMode::Disabled),
                    ..Default::default()
                }),
                temperature: Some(0.7),
                ..Default::default()
            },
            tools: None,
            response_format: None,
            session_id: None,
            transport: None,
        };
        let body = provider.build_request_body(&request, false);
        assert!(body.get("thinking").is_none());
        assert_eq!(body["temperature"], 0.7);
    }

    #[test]
    fn request_body_includes_thinking_blocks_in_messages() {
        let provider =
            AnthropicProvider::new(AnthropicModel::ClaudeSonnet4, "test-key".to_string(), None);
        let messages = vec![
            ModelMessage::user("hello"),
            ModelMessage {
                role: Role::Assistant,
                content: vec![
                    ContentPart::Thinking(ThinkingContent {
                        thinking: "Let me think...".to_string(),
                        signature: "sig123".to_string(),
                    }),
                    ContentPart::Text {
                        text: "Here's my answer".to_string(),
                    },
                ],
                name: None,
                timestamp: None,
            },
        ];
        let request = ProviderRequest {
            messages,
            settings: GenerationSettings {
                anthropic: Some(AnthropicOptions {
                    thinking: Some(ThinkingMode::Enabled {
                        budget_tokens: 10000,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            tools: None,
            response_format: None,
            session_id: None,
            transport: None,
        };
        let body = provider.build_request_body(&request, false);
        let assistant_content = &body["messages"][1]["content"];
        assert_eq!(assistant_content[0]["type"], "thinking");
        assert_eq!(assistant_content[0]["thinking"], "Let me think...");
        assert_eq!(assistant_content[0]["signature"], "sig123");
        assert_eq!(assistant_content[1]["type"], "text");
    }

    #[test]
    fn request_body_excludes_thinking_blocks_when_not_enabled() {
        let provider =
            AnthropicProvider::new(AnthropicModel::ClaudeSonnet4, "test-key".to_string(), None);
        let messages = vec![
            ModelMessage::user("hello"),
            ModelMessage {
                role: Role::Assistant,
                content: vec![
                    ContentPart::Thinking(ThinkingContent {
                        thinking: "Let me think...".to_string(),
                        signature: "sig123".to_string(),
                    }),
                    ContentPart::Text {
                        text: "Here's my answer".to_string(),
                    },
                ],
                name: None,
                timestamp: None,
            },
        ];
        let request = ProviderRequest {
            messages,
            settings: settings(),
            tools: None,
            response_format: None,
            session_id: None,
            transport: None,
        };
        let body = provider.build_request_body(&request, false);
        let assistant_content = &body["messages"][1]["content"];
        // Only text block should be present, no thinking
        assert_eq!(assistant_content.as_array().unwrap().len(), 1);
        assert_eq!(assistant_content[0]["type"], "text");
    }

    #[test]
    fn beta_headers_always_sent() {
        let provider =
            AnthropicProvider::new(AnthropicModel::ClaudeSonnet4, "test-key".to_string(), None);
        let headers = provider.build_headers();
        let beta = headers.get("anthropic-beta").unwrap().to_str().unwrap();
        assert!(beta.contains("interleaved-thinking-2025-05-14"));
        assert!(beta.contains("fine-grained-tool-streaming-2025-05-14"));
    }

    #[test]
    fn tool_choice_serialization() {
        let provider =
            AnthropicProvider::new(AnthropicModel::ClaudeSonnet4, "test-key".to_string(), None);
        let tools = vec![ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get weather".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }];

        // auto
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: GenerationSettings {
                tool_choice: Some(ToolChoice::Auto),
                ..Default::default()
            },
            tools: Some(tools.clone()),
            response_format: None,
            session_id: None,
            transport: None,
        };
        let body = provider.build_request_body(&request, false);
        assert_eq!(body["tool_choice"]["type"], "auto");

        // required → "any" for Anthropic
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: GenerationSettings {
                tool_choice: Some(ToolChoice::Required),
                ..Default::default()
            },
            tools: Some(tools.clone()),
            response_format: None,
            session_id: None,
            transport: None,
        };
        let body = provider.build_request_body(&request, false);
        assert_eq!(body["tool_choice"]["type"], "any");

        // specific function
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: GenerationSettings {
                tool_choice: Some(ToolChoice::Function("get_weather".to_string())),
                ..Default::default()
            },
            tools: Some(tools),
            response_format: None,
            session_id: None,
            transport: None,
        };
        let body = provider.build_request_body(&request, false);
        assert_eq!(body["tool_choice"]["type"], "tool");
        assert_eq!(body["tool_choice"]["name"], "get_weather");
    }
}
