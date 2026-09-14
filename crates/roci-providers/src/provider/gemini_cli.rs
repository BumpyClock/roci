//! Native Cloud Code Assist transport used by Gemini CLI OAuth accounts.

use async_trait::async_trait;
use futures::{stream::BoxStream, StreamExt};
use roci_core::auth::Token;
use roci_core::error::RociError;
use roci_core::models::capabilities::ModelCapabilities;
use roci_core::provider::{ModelProvider, ProviderRequest, ProviderResponse};
use roci_core::types::*;
use serde_json::{json, Value};

use super::google::GoogleProvider;
use crate::auth::gemini::{project_id, CLOUD_CODE_ENDPOINT};
use crate::models::google::GoogleModel;

pub struct GeminiCliProvider {
    serializer: GoogleProvider,
    access_token: String,
    project_id: String,
    endpoint: String,
    client: reqwest::Client,
}

impl GeminiCliProvider {
    pub fn new(model: GoogleModel, token: &Token) -> Result<Self, RociError> {
        let project = project_id(token)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                RociError::Authentication(
                    "Gemini project is missing; run Gemini login again".into(),
                )
            })?;
        if token.access_token.trim().is_empty() {
            return Err(RociError::Authentication(
                "Gemini OAuth token is missing".into(),
            ));
        }
        Ok(Self {
            serializer: GoogleProvider::new(model, String::new(), None),
            access_token: token.access_token.clone(),
            project_id: project.into(),
            endpoint: CLOUD_CODE_ENDPOINT.into(),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("valid Gemini provider client"),
        })
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into().trim_end_matches('/').into();
        self
    }

    async fn send(
        &self,
        request: &ProviderRequest,
        streaming: bool,
    ) -> Result<reqwest::Response, RociError> {
        self.serializer.validate_settings(&request.settings)?;
        let mut body = json!({"model": self.serializer.api_model_id(), "project": self.project_id, "request": self.serializer.build_request_body(request)});
        if let Some(session_id) = &request.session_id {
            body["request"]["session_id"] = json!(session_id);
        }
        if let Some(callback) = &request.payload_callback {
            callback(body.clone());
        }
        let action = if streaming {
            "streamGenerateContent?alt=sse"
        } else {
            "generateContent"
        };
        let response = self
            .client
            .post(format!("{}/v1internal:{action}", self.endpoint))
            .headers(request.headers.clone())
            .bearer_auth(
                request
                    .api_key_override
                    .as_deref()
                    .unwrap_or(&self.access_token),
            )
            .header(
                "User-Agent",
                format!("GeminiCLI/0.34.0/{} (roci)", self.model_id()),
            )
            .header(
                "X-Goog-Api-Client",
                "google-genai-sdk/1.41.0 gl-node/v22.19.0",
            )
            .header(
                "Accept",
                if streaming {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .json(&body)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(RociError::api(
                response.status().as_u16(),
                "Gemini Cloud Code Assist request failed",
            ));
        }
        Ok(response)
    }
}

#[async_trait]
impl ModelProvider for GeminiCliProvider {
    fn provider_name(&self) -> &str {
        "google"
    }
    fn model_id(&self) -> &str {
        self.serializer.model_id()
    }
    fn capabilities(&self) -> &ModelCapabilities {
        self.serializer.capabilities()
    }

    async fn generate_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        let response = self.send(request, false).await?;
        let envelope: Value = response.json().await?;
        let mut state = StreamState::default();
        let deltas = state.decode(&envelope)?;
        if envelope["response"]["candidates"]
            .as_array()
            .is_none_or(Vec::is_empty)
        {
            return Err(protocol_error("response contains no candidates"));
        }
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        let mut thinking = Vec::new();
        for delta in deltas {
            text.push_str(&delta.text);
            if let Some(call) = delta.tool_call {
                tool_calls.push(call);
            }
            if let Some(reasoning) = delta.reasoning {
                thinking.push(ContentPart::Thinking(ThinkingContent {
                    thinking: reasoning,
                    signature: delta.reasoning_signature.unwrap_or_default(),
                }));
            }
        }
        Ok(ProviderResponse {
            text,
            tool_calls,
            thinking,
            usage: state.usage.unwrap_or_default(),
            finish_reason: if state.tools {
                Some(FinishReason::ToolCalls)
            } else {
                state.finish
            },
        })
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        let response = self.send(request, true).await?;
        let mut bytes = response.bytes_stream();
        Ok(Box::pin(async_stream::try_stream! {
            let mut frames = SseFrames::default();
            let mut state = StreamState::default();
            while let Some(chunk) = bytes.next().await {
                for frame in frames.push(&chunk?, false)? {
                    if frame == "[DONE]" { continue; }
                    let envelope: Value = serde_json::from_str(&frame).map_err(|_| protocol_error("invalid stream JSON"))?;
                    for delta in state.decode(&envelope)? { yield delta; }
                }
            }
            for frame in frames.push(&[], true)? {
                if frame == "[DONE]" { continue; }
                let envelope: Value = serde_json::from_str(&frame).map_err(|_| protocol_error("invalid stream JSON"))?;
                for delta in state.decode(&envelope)? { yield delta; }
            }
            if !state.seen_response { Err(protocol_error("stream ended without a response"))?; }
            let mut done = empty_delta(StreamEventType::Done);
            done.finish_reason = if state.tools { Some(FinishReason::ToolCalls) } else { state.finish };
            done.usage = state.usage;
            yield done;
        }))
    }
}

fn protocol_error(message: &str) -> RociError {
    RociError::api(200, format!("Gemini {message}"))
}

fn empty_delta(event_type: StreamEventType) -> TextStreamDelta {
    TextStreamDelta {
        text: String::new(),
        event_type,
        tool_call: None,
        finish_reason: None,
        usage: None,
        reasoning: None,
        reasoning_signature: None,
        reasoning_type: None,
    }
}

#[derive(Default)]
struct StreamState {
    seen_response: bool,
    usage: Option<Usage>,
    finish: Option<FinishReason>,
    tools: bool,
}

impl StreamState {
    fn decode(&mut self, envelope: &Value) -> Result<Vec<TextStreamDelta>, RociError> {
        if let Some(error) = envelope.get("error").or_else(|| {
            envelope
                .get("response")
                .and_then(|value| value.get("error"))
        }) {
            let status = error["code"]
                .as_u64()
                .and_then(|code| u16::try_from(code).ok())
                .filter(|code| (400..600).contains(code))
                .unwrap_or(500);
            return Err(RociError::api(status, "Gemini stream request failed"));
        }
        let response = envelope
            .get("response")
            .filter(|value| value.is_object())
            .ok_or_else(|| protocol_error("missing response envelope"))?;
        self.seen_response = true;
        if let Some(usage) = response.get("usageMetadata") {
            let count = |key: &str| usage[key].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32;
            self.usage = Some(Usage {
                input_tokens: count("promptTokenCount"),
                output_tokens: count("candidatesTokenCount"),
                total_tokens: count("totalTokenCount"),
                cache_read_tokens: usage
                    .get("cachedContentTokenCount")
                    .map(|_| count("cachedContentTokenCount")),
                reasoning_tokens: usage
                    .get("thoughtsTokenCount")
                    .map(|_| count("thoughtsTokenCount")),
                ..Default::default()
            });
        }
        let mut deltas = Vec::new();
        if let Some(candidate) = response["candidates"]
            .as_array()
            .and_then(|values| values.first())
        {
            self.finish = match candidate["finishReason"].as_str() {
                Some("STOP") => Some(FinishReason::Stop),
                Some("MAX_TOKENS") => Some(FinishReason::Length),
                Some("SAFETY" | "RECITATION" | "PROHIBITED_CONTENT") => {
                    Some(FinishReason::ContentFilter)
                }
                _ => self.finish,
            };
            for part in candidate["content"]["parts"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if let Some(text) = part["text"].as_str() {
                    let thought = part["thought"].as_bool() == Some(true);
                    let mut delta = empty_delta(if thought {
                        StreamEventType::Reasoning
                    } else {
                        StreamEventType::TextDelta
                    });
                    if thought {
                        delta.reasoning = Some(text.into());
                        delta.reasoning_signature =
                            part["thoughtSignature"].as_str().map(str::to_string);
                        delta.reasoning_type = Some("thinking".into());
                    } else {
                        delta.text = text.into();
                    }
                    deltas.push(delta);
                }
                if let Some(call) = part.get("functionCall") {
                    let name = call["name"]
                        .as_str()
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| protocol_error("tool call has no name"))?;
                    let mut delta = empty_delta(StreamEventType::ToolCallDelta);
                    delta.tool_call = Some(AgentToolCall {
                        id: call["id"]
                            .as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                        name: name.into(),
                        arguments: call.get("args").cloned().unwrap_or_else(|| json!({})),
                        called_as: None,
                        recipient: part["thoughtSignature"].as_str().map(str::to_string),
                    });
                    self.tools = true;
                    deltas.push(delta);
                }
            }
        }
        Ok(deltas)
    }
}

#[derive(Default)]
struct SseFrames {
    buffer: Vec<u8>,
    data: Vec<String>,
}

impl SseFrames {
    fn push(&mut self, chunk: &[u8], eof: bool) -> Result<Vec<String>, RociError> {
        const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
        self.buffer.extend_from_slice(chunk);
        if self.buffer.len() + self.data.iter().map(String::len).sum::<usize>() > MAX_FRAME_BYTES {
            return Err(protocol_error("stream frame exceeds size limit"));
        }
        let mut frames = Vec::new();
        while let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<_> = self.buffer.drain(..=end).collect();
            self.line(&line[..line.len() - 1], &mut frames)?;
        }
        if eof {
            let line = std::mem::take(&mut self.buffer);
            if !line.is_empty() {
                self.line(&line, &mut frames)?;
            }
            self.line(b"", &mut frames)?;
        }
        Ok(frames)
    }

    fn line(&mut self, line: &[u8], frames: &mut Vec<String>) -> Result<(), RociError> {
        let line = std::str::from_utf8(line)
            .map_err(|_| protocol_error("invalid stream UTF-8"))?
            .trim_end_matches('\r');
        if line.is_empty() {
            if !self.data.is_empty() {
                frames.push(std::mem::take(&mut self.data).join("\n"));
            }
        } else if let Some(data) = line.strip_prefix("data:") {
            self.data
                .push(data.strip_prefix(' ').unwrap_or(data).into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
