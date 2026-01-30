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

    fn build_request_body(&self, request: &ProviderRequest) -> serde_json::Value {
        let mut system_instruction = None;
        let mut contents = Vec::new();

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
                    contents.push(serde_json::json!({
                        "role": "model",
                        "parts": [{"text": msg.text()}],
                    }));
                }
                Role::Tool => {
                    for part in &msg.content {
                        if let ContentPart::ToolResult(tr) = part {
                            contents.push(serde_json::json!({
                                "role": "function",
                                "parts": [{
                                    "functionResponse": {
                                        "name": tr.tool_call_id,
                                        "response": tr.result,
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

        let mut gen_config = serde_json::Map::new();
        if let Some(max) = request.settings.max_tokens {
            gen_config.insert("maxOutputTokens".into(), max.into());
        }
        if let Some(temp) = request.settings.temperature {
            gen_config.insert("temperature".into(), temp.into());
        }
        if let Some(top_p) = request.settings.top_p {
            gen_config.insert("topP".into(), top_p.into());
        }
        if let Some(ref stops) = request.settings.stop_sequences {
            gen_config.insert("stopSequences".into(), serde_json::json!(stops));
        }
        if !gen_config.is_empty() {
            obj.insert("generationConfig".into(), serde_json::Value::Object(gen_config));
        }

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
    fn model_id(&self) -> &str {
        self.model.as_str()
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(&self, request: &ProviderRequest) -> Result<ProviderResponse, RociError> {
        let body = self.build_request_body(request);
        let url = format!(
            "{}/models/{}:generateContent?key={}",
            BASE_URL,
            self.model.as_str(),
            self.api_key
        );

        debug!(model = self.model.as_str(), "Google generate_text");

        let resp = shared_client()
            .post(&url)
            .json(&body)
            .send()
            .await?;

        let status = resp.status().as_u16();
        if status != 200 {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(super::http::status_to_error(status, &body_text));
        }

        let data: GeminiResponse = resp.json().await?;

        let candidate = data.candidates.into_iter().next().ok_or_else(|| {
            RociError::api(200, "No candidates in Gemini response")
        })?;

        let mut text = String::new();
        let mut tool_calls = Vec::new();

        for part in candidate.content.parts {
            if let Some(t) = part.text {
                text.push_str(&t);
            }
            if let Some(fc) = part.function_call {
                tool_calls.push(message::AgentToolCall {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: fc.name,
                    arguments: fc.args.unwrap_or(serde_json::Value::Object(Default::default())),
                });
            }
        }

        let finish_reason = match candidate.finish_reason.as_deref() {
            Some("STOP") => Some(FinishReason::Stop),
            Some("MAX_TOKENS") => Some(FinishReason::Length),
            Some("SAFETY") => Some(FinishReason::ContentFilter),
            _ => None,
        };

        let usage = data.usage_metadata.map(|u| Usage {
            input_tokens: u.prompt_token_count,
            output_tokens: u.candidates_token_count,
            total_tokens: u.total_token_count,
            ..Default::default()
        }).unwrap_or_default();

        Ok(ProviderResponse {
            text,
            usage,
            tool_calls,
            finish_reason,
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
            self.model.as_str(),
            self.api_key
        );

        debug!(model = self.model.as_str(), "Google stream_text");

        let resp = shared_client()
            .post(&url)
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

                    if let Some(data) = super::http::parse_sse_data(&line) {
                        if let Ok(resp) = serde_json::from_str::<GeminiResponse>(data) {
                            if let Some(candidate) = resp.candidates.into_iter().next() {
                                for part in candidate.content.parts {
                                    if let Some(t) = part.text {
                                        let done = candidate.finish_reason.is_some();
                                        let finish = if done {
                                            Some(FinishReason::Stop)
                                        } else {
                                            None
                                        };
                                        yield Ok(TextStreamDelta {
                                            text: t,
                                            event_type: if done { StreamEventType::Done } else { StreamEventType::TextDelta },
                                            finish_reason: finish,
                                            usage: resp.usage_metadata.as_ref().map(|u| Usage {
                                                input_tokens: u.prompt_token_count,
                                                output_tokens: u.candidates_token_count,
                                                total_tokens: u.total_token_count,
                                                ..Default::default()
                                            }),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
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
}

#[derive(Deserialize)]
struct GeminiFunctionCall {
    name: String,
    args: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsage {
    prompt_token_count: u32,
    candidates_token_count: u32,
    total_token_count: u32,
}
