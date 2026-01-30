//! Google Gemini API provider.

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use serde::Deserialize;
use tracing::debug;

use crate::error::RociError;
use crate::models::capabilities::ModelCapabilities;
use crate::models::google::GoogleModel;
use crate::types::*;

use super::http::shared_client;
use super::{ModelProvider, ProviderRequest, ProviderResponse};

const BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct GoogleProvider {
    model: GoogleModel,
    api_key: String,
    capabilities: ModelCapabilities,
}

impl GoogleProvider {
    pub fn new(model: GoogleModel, api_key: String) -> Self {
        let capabilities = model.capabilities();
        Self {
            model,
            api_key,
            capabilities,
        }
    }

    fn api_model_id(&self) -> &str {
        match self.model {
            GoogleModel::Gemini3Flash => "gemini-3-flash-preview",
            _ => self.model.as_str(),
        }
    }

    fn build_request_body(&self, request: &ProviderRequest) -> serde_json::Value {
        let mut system_instruction = None;
        let mut contents = Vec::new();
        let mut tool_name_map = std::collections::HashMap::new();

        for msg in &request.messages {
            for part in &msg.content {
                if let ContentPart::ToolCall(tc) = part {
                    tool_name_map.insert(tc.id.clone(), tc.name.clone());
                }
            }
        }

        for msg in &request.messages {
            match msg.role {
                Role::System => {
                    system_instruction = Some(serde_json::json!({
                        "parts": [{"text": msg.text()}]
                    }));
                }
                Role::User => {
                    let parts = build_gemini_parts(&msg.content);
                    contents.push(serde_json::json!({
                        "role": "user",
                        "parts": parts,
                    }));
                }
                Role::Assistant => {
                    let mut parts = Vec::new();
                    for part in &msg.content {
                        match part {
                            ContentPart::Text { text } => {
                                parts.push(serde_json::json!({ "text": text }))
                            }
                            ContentPart::Image(img) => parts.push(serde_json::json!({
                                "inlineData": {
                                    "mimeType": img.mime_type,
                                    "data": img.data,
                                }
                            })),
                            ContentPart::ToolCall(tc) => {
                                let mut part = serde_json::json!({
                                    "functionCall": {
                                        "id": tc.id,
                                        "name": tc.name,
                                        "args": tc.arguments.clone(),
                                    }
                                });
                                if let Some(ref recipient) = tc.recipient {
                                    if let Some(obj) = part.as_object_mut() {
                                        obj.insert(
                                            "thoughtSignature".into(),
                                            recipient.clone().into(),
                                        );
                                    }
                                }
                                parts.push(part);
                            }
                            ContentPart::ToolResult(_) => {}
                            ContentPart::Thinking(_) => {}
                            ContentPart::RedactedThinking(_) => {}
                        }
                    }
                    if !parts.is_empty() {
                        contents.push(serde_json::json!({
                            "role": "model",
                            "parts": parts,
                        }));
                    }
                }
                Role::Tool => {
                    for part in &msg.content {
                        if let ContentPart::ToolResult(tr) = part {
                            let name = tool_name_map
                                .get(&tr.tool_call_id)
                                .cloned()
                                .unwrap_or_else(|| tr.tool_call_id.clone());
                            contents.push(serde_json::json!({
                                "role": "tool",
                                "parts": [{
                                    "functionResponse": {
                                        "id": tr.tool_call_id,
                                        "name": name,
                                        "response": tr.result.clone(),
                                    }
                                }]
                            }));
                        }
                    }
                }
            }
        }

        let mut body = serde_json::json!({ "contents": contents });
        let obj = body.as_object_mut().unwrap();

        if let Some(sys) = system_instruction {
            obj.insert("systemInstruction".into(), sys);
        }

        let max_tokens = request.settings.max_tokens.unwrap_or(2048);
        let temperature = request.settings.temperature.unwrap_or(0.7);
        let top_p = request.settings.top_p.unwrap_or(0.95);
        let top_k = request.settings.top_k.unwrap_or(40);

        let mut gen_config = serde_json::Map::new();
        gen_config.insert("maxOutputTokens".into(), max_tokens.into());
        gen_config.insert("temperature".into(), temperature.into());
        gen_config.insert("topP".into(), top_p.into());
        gen_config.insert("topK".into(), top_k.into());
        if let Some(ref stops) = request.settings.stop_sequences {
            gen_config.insert("stopSequences".into(), serde_json::json!(stops));
        }
        if let Some(ref fmt) = request.response_format {
            match fmt {
                ResponseFormat::JsonObject => {
                    gen_config.insert("responseMimeType".into(), "application/json".into());
                }
                ResponseFormat::JsonSchema { schema, .. } => {
                    gen_config.insert("responseMimeType".into(), "application/json".into());
                    gen_config.insert("responseJsonSchema".into(), schema.clone());
                }
                ResponseFormat::Text => {}
            }
        }

        obj.insert(
            "generationConfig".into(),
            serde_json::Value::Object(gen_config),
        );

        if let Some(ref tools) = request.tools {
            if !tools.is_empty() {
                let fn_decls: Vec<serde_json::Value> = tools
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        })
                    })
                    .collect();
                obj.insert(
                    "tools".into(),
                    serde_json::json!([{"functionDeclarations": fn_decls}]),
                );
            }
        }

        body
    }
}

#[async_trait]
impl ModelProvider for GoogleProvider {
    fn provider_name(&self) -> &str {
        "google"
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
        let body = self.build_request_body(request);
        let url = format!(
            "{}/models/{}:generateContent?key={}",
            BASE_URL,
            self.api_model_id(),
            self.api_key
        );

        debug!(model = self.model.as_str(), "Google generate_text");

        let resp = shared_client().post(&url).json(&body).send().await?;

        let status = resp.status().as_u16();
        if status != 200 {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(super::http::status_to_error(status, &body_text));
        }

        let data: GeminiResponse = resp.json().await?;

        let candidate = data
            .candidates
            .into_iter()
            .next()
            .ok_or_else(|| RociError::api(200, "No candidates in Gemini response"))?;

        let mut text = String::new();
        let mut tool_calls = Vec::new();

        for part in candidate.content.parts {
            let GeminiPart {
                text: part_text,
                function_call,
                thought_signature,
            } = part;
            if let Some(t) = part_text {
                text.push_str(&t);
            }
            if let Some(fc) = function_call {
                let id = fc.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                tool_calls.push(AgentToolCall {
                    id,
                    name: fc.name,
                    arguments: fc
                        .args
                        .unwrap_or(serde_json::Value::Object(Default::default())),
                    recipient: thought_signature,
                });
            }
        }

        let finish_reason = match candidate.finish_reason.as_deref() {
            Some("STOP") => Some(FinishReason::Stop),
            Some("MAX_TOKENS") => Some(FinishReason::Length),
            Some("SAFETY") => Some(FinishReason::ContentFilter),
            _ => None,
        };

        let usage = data
            .usage_metadata
            .map(|u| Usage {
                input_tokens: u.prompt_token_count,
                output_tokens: u.candidates_token_count,
                total_tokens: u.total_token_count,
                ..Default::default()
            })
            .unwrap_or_default();

        Ok(ProviderResponse {
            text,
            usage,
            tool_calls,
            finish_reason,
            thinking: Vec::new(),
        })
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        let body = self.build_request_body(request);
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse&key={}",
            BASE_URL,
            self.api_model_id(),
            self.api_key
        );

        debug!(model = self.model.as_str(), "Google stream_text");

        let resp = shared_client().post(&url).json(&body).send().await?;

        let status = resp.status().as_u16();
        if status != 200 {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(super::http::status_to_error(status, &body_text));
        }

        let byte_stream = resp.bytes_stream();

        let stream = async_stream::stream! {
            let mut buffer = String::new();
            let mut saw_tool_call = false;
            let mut finish_reason: Option<FinishReason> = None;
            let mut usage: Option<Usage> = None;
            futures::pin_mut!(byte_stream);

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        yield Err(RociError::Network(e));
                        return;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if let Some(data) = super::http::parse_sse_data(&line) {
                        if let Ok(resp) = serde_json::from_str::<GeminiResponse>(data) {
                            let GeminiResponse { candidates, usage_metadata } = resp;
                            if let Some(candidate) = candidates.into_iter().next() {
                                for part in candidate.content.parts {
                                    let GeminiPart { text: part_text, function_call, thought_signature } = part;
                                    if let Some(call) = function_call {
                                        saw_tool_call = true;
                                        let id = call.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                                        let args = call.args.unwrap_or(serde_json::Value::Object(Default::default()));
                                        yield Ok(TextStreamDelta {
                                            text: String::new(),
                                            event_type: StreamEventType::ToolCallDelta,
                                            tool_call: Some(AgentToolCall { id, name: call.name, arguments: args, recipient: thought_signature }),
                                            finish_reason: None,
                                            usage: None,
                                            reasoning: None,
                                            reasoning_signature: None,
                                            reasoning_type: None,
                                        });
                                    }
                                    if let Some(t) = part_text {
                                        yield Ok(TextStreamDelta {
                                            text: t,
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
                                if let Some(reason) = candidate.finish_reason.as_deref() {
                                    finish_reason = match reason {
                                        "STOP" => Some(FinishReason::Stop),
                                        "MAX_TOKENS" => Some(FinishReason::Length),
                                        "SAFETY" => Some(FinishReason::ContentFilter),
                                        _ => finish_reason,
                                    };
                                }
                            }
                            if let Some(meta) = usage_metadata {
                                usage = Some(Usage {
                                    input_tokens: meta.prompt_token_count,
                                    output_tokens: meta.candidates_token_count,
                                    total_tokens: meta.total_token_count,
                                    ..Default::default()
                                });
                            }
                        }
                    }
                }
            }

            let done_reason = if saw_tool_call { Some(FinishReason::ToolCalls) } else { finish_reason };
            yield Ok(TextStreamDelta {
                text: String::new(),
                event_type: StreamEventType::Done,
                tool_call: None,
                finish_reason: done_reason,
                usage,
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            });
        };

        Ok(Box::pin(stream))
    }
}

fn build_gemini_parts(content: &[ContentPart]) -> Vec<serde_json::Value> {
    content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(serde_json::json!({"text": text})),
            ContentPart::Image(img) => Some(serde_json::json!({
                "inlineData": {
                    "mimeType": img.mime_type,
                    "data": img.data,
                }
            })),
            _ => None,
        })
        .collect()
}

// Internal Gemini response types

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    usage_metadata: Option<GeminiUsage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: GeminiContent,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiPart {
    text: Option<String>,
    function_call: Option<GeminiFunctionCall>,
    thought_signature: Option<String>,
}

#[derive(Deserialize)]
struct GeminiFunctionCall {
    #[serde(default)]
    id: Option<String>,
    name: String,
    args: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsage {
    prompt_token_count: u32,
    #[serde(default)]
    candidates_token_count: u32,
    #[serde(default)]
    total_token_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(
        max_tokens: Option<u32>,
        temperature: Option<f64>,
        top_p: Option<f64>,
        top_k: Option<u32>,
    ) -> GenerationSettings {
        GenerationSettings {
            max_tokens,
            temperature,
            top_p,
            top_k,
            stop_sequences: None,
            presence_penalty: None,
            frequency_penalty: None,
            seed: None,
            reasoning_effort: None,
            text_verbosity: None,
            response_format: None,
            openai_responses: None,
            user: None,
            anthropic: None,
            tool_choice: None,
        }
    }

    #[test]
    fn build_request_body_includes_thought_signature_and_call_id() {
        let provider =
            GoogleProvider::new(GoogleModel::Gemini3FlashPreview, "test-key".to_string());
        let tool_call = AgentToolCall {
            id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: serde_json::json!({"city": "Paris"}),
            recipient: Some("sig".to_string()),
        };
        let messages = vec![ModelMessage {
            role: Role::Assistant,
            content: vec![ContentPart::ToolCall(tool_call)],
            name: None,
            timestamp: None,
        }];
        let request = ProviderRequest {
            messages,
            settings: GenerationSettings::default(),
            tools: None,
            response_format: None,
        };
        let body = provider.build_request_body(&request);
        assert_eq!(
            body["contents"][0]["parts"][0]["functionCall"]["id"],
            "call_1"
        );
        assert_eq!(body["contents"][0]["parts"][0]["thoughtSignature"], "sig");
    }

    #[test]
    fn build_request_body_includes_function_response_id() {
        let provider =
            GoogleProvider::new(GoogleModel::Gemini3FlashPreview, "test-key".to_string());
        let tool_call = AgentToolCall {
            id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: serde_json::json!({"city": "Paris"}),
            recipient: None,
        };
        let messages = vec![
            ModelMessage {
                role: Role::Assistant,
                content: vec![ContentPart::ToolCall(tool_call)],
                name: None,
                timestamp: None,
            },
            ModelMessage::tool_result("call_1", serde_json::json!({"temp": 18}), false),
        ];
        let request = ProviderRequest {
            messages,
            settings: GenerationSettings::default(),
            tools: None,
            response_format: None,
        };
        let body = provider.build_request_body(&request);
        assert_eq!(
            body["contents"][1]["parts"][0]["functionResponse"]["id"],
            "call_1"
        );
        assert_eq!(body["contents"][1]["role"], "tool");
        assert_eq!(
            body["contents"][1]["parts"][0]["functionResponse"]["name"],
            "get_weather"
        );
    }

    #[test]
    fn build_request_body_includes_response_json_schema() {
        let provider =
            GoogleProvider::new(GoogleModel::Gemini3FlashPreview, "test-key".to_string());
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("Return JSON")],
            settings: GenerationSettings {
                response_format: Some(ResponseFormat::JsonSchema {
                    schema: serde_json::json!({
                        "type": "object",
                        "properties": { "ok": { "type": "boolean" } },
                        "required": ["ok"]
                    }),
                    name: "Flag".to_string(),
                }),
                ..Default::default()
            },
            tools: None,
            response_format: Some(ResponseFormat::JsonSchema {
                schema: serde_json::json!({
                    "type": "object",
                    "properties": { "ok": { "type": "boolean" } },
                    "required": ["ok"]
                }),
                name: "Flag".to_string(),
            }),
        };
        let body = provider.build_request_body(&request);
        assert_eq!(
            body["generationConfig"]["responseMimeType"],
            "application/json"
        );
        assert!(body["generationConfig"]["responseJsonSchema"].is_object());
    }

    #[test]
    fn build_request_body_defaults_generation_config() {
        let provider =
            GoogleProvider::new(GoogleModel::Gemini3FlashPreview, "test-key".to_string());
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(None, None, None, None),
            tools: None,
            response_format: None,
        };
        let body = provider.build_request_body(&request);
        assert_eq!(
            body["generationConfig"]["maxOutputTokens"].as_u64(),
            Some(2048)
        );
        assert_eq!(body["generationConfig"]["temperature"].as_f64(), Some(0.7));
        assert_eq!(body["generationConfig"]["topP"].as_f64(), Some(0.95));
        assert_eq!(body["generationConfig"]["topK"].as_u64(), Some(40));
    }

    #[test]
    fn build_request_body_respects_generation_config_overrides() {
        let provider =
            GoogleProvider::new(GoogleModel::Gemini3FlashPreview, "test-key".to_string());
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(Some(1200), Some(0.3), Some(0.5), Some(12)),
            tools: None,
            response_format: None,
        };
        let body = provider.build_request_body(&request);
        assert_eq!(
            body["generationConfig"]["maxOutputTokens"].as_u64(),
            Some(1200)
        );
        assert_eq!(body["generationConfig"]["temperature"].as_f64(), Some(0.3));
        assert_eq!(body["generationConfig"]["topP"].as_f64(), Some(0.5));
        assert_eq!(body["generationConfig"]["topK"].as_u64(), Some(12));
    }
}
