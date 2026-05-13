//! OpenAI Responses API provider (for GPT-5, o3, o4-mini, etc.)

mod errors;
mod headers;
mod request;
pub(crate) mod response;
mod stream;

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use tracing::debug;

use crate::models::openai::OpenAiModel;
use roci_core::error::RociError;
use roci_core::models::capabilities::ModelCapabilities;
use roci_core::types::*;
use roci_core::util::debug::roci_debug_enabled;

use roci_core::provider::http::shared_client;
use roci_core::provider::{ModelProvider, ProviderRequest, ProviderResponse};

use errors::success_or_openai_error;
use response::ResponsesApiResponse;
use stream::{extract_response_error, tool_call_delta, StreamToolCallState};

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

pub struct OpenAiResponsesProvider {
    model: OpenAiModel,
    api_key: String,
    base_url: String,
    account_id: Option<String>,
    capabilities: ModelCapabilities,
    is_codex: bool,
}

impl OpenAiResponsesProvider {
    pub fn new(
        model: OpenAiModel,
        api_key: String,
        base_url: Option<String>,
        account_id: Option<String>,
    ) -> Self {
        let capabilities = model.capabilities();
        let base_url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let is_codex = base_url.contains("chatgpt.com/backend-api/codex");
        Self {
            base_url,
            model,
            api_key,
            account_id,
            capabilities,
            is_codex,
        }
    }
}

#[async_trait]
impl ModelProvider for OpenAiResponsesProvider {
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
        self.validate_settings(&request.settings)?;
        let body = self.build_request_body(request, false);
        self.emit_payload_callback(request, &body);
        let url = self.responses_url(request);

        debug!(
            model = self.model.as_str(),
            "OpenAI Responses generate_text"
        );

        let resp = shared_client()
            .post(&url)
            .headers(self.build_headers(request)?)
            .json(&body)
            .send()
            .await?;

        let resp = success_or_openai_error(resp).await?;

        let payload: serde_json::Value = resp.json().await?;
        let data: ResponsesApiResponse = serde_json::from_value(payload)?;
        Self::parse_response(data)
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        self.validate_settings(&request.settings)?;
        let body = self.build_request_body(request, true);
        self.emit_payload_callback(request, &body);
        let url = self.responses_url(request);

        debug!(model = self.model.as_str(), "OpenAI Responses stream_text");

        let resp = shared_client()
            .post(&url)
            .headers(self.build_headers(request)?)
            .json(&body)
            .send()
            .await?;

        let resp = success_or_openai_error(resp).await?;

        let byte_stream = resp.bytes_stream();

        let stream = async_stream::stream! {
            let mut buffer = String::new();
            let mut tool_call_state = StreamToolCallState::default();
            let mut saw_tool_call = false;
            let mut saw_text_delta = false;
            let mut pending_data: Vec<String> = Vec::new();
            let mut saw_done = false;
            let mut debug_event_count = 0usize;
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
                    let line = buffer[..line_end].to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    let line = line.trim_end_matches('\r');
                    if line.is_empty() {
                        if pending_data.is_empty() {
                            continue;
                        }
                        let data = pending_data.join("\n");
                        pending_data.clear();
                        if data == "[DONE]" {
                            saw_done = true;
                            break;
                        }
                        if roci_debug_enabled() && debug_event_count < 5 {
                            tracing::debug!(data = %data, "OpenAI Responses SSE raw");
                            debug_event_count += 1;
                        }
                        match serde_json::from_str::<serde_json::Value>(&data) {
                            Ok(event) => {
                                let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                match event_type {
                                    "response.output_item.added" => {
                                        if let Some(item) = event.get("item") {
                                            if item.get("type").and_then(|t| t.as_str()) == Some("message")
                                                && !saw_text_delta
                                            {
                                                if let Some(content) =
                                                    item.get("content").and_then(|v| v.as_array())
                                                {
                                                    for part in content {
                                                        if part.get("type").and_then(|t| t.as_str())
                                                            == Some("output_text")
                                                        {
                                                            if let Some(text) =
                                                                part.get("text").and_then(|t| t.as_str())
                                                            {
                                                                if !text.is_empty() {
                                                                    saw_text_delta = true;
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
                                                        }
                                                    }
                                                }
                                            }
                                            if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                                                if let Some(id) = item
                                                    .get("call_id")
                                                    .and_then(|v| v.as_str())
                                                    .or_else(|| item.get("id").and_then(|v| v.as_str()))
                                                {
                                                    tool_call_state.observe_call(
                                                        id,
                                                        item.get("name").and_then(|v| v.as_str()),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    "response.output_item.done" => {
                                        if let Some(item) = event.get("item") {
                                            if item.get("type").and_then(|t| t.as_str()) == Some("message")
                                                && !saw_text_delta
                                            {
                                                if let Some(content) =
                                                    item.get("content").and_then(|v| v.as_array())
                                                {
                                                    let mut completed_text = String::new();
                                                    for part in content {
                                                        if part.get("type").and_then(|t| t.as_str())
                                                            == Some("output_text")
                                                        {
                                                            if let Some(text) =
                                                                part.get("text").and_then(|t| t.as_str())
                                                            {
                                                                completed_text.push_str(text);
                                                            }
                                                        }
                                                    }
                                                    if !completed_text.trim().is_empty() {
                                                        saw_text_delta = true;
                                                        yield Ok(TextStreamDelta {
                                                            text: completed_text,
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
                                            }
                                            if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                                                if let Some(call_id) = item
                                                    .get("call_id")
                                                    .and_then(|v| v.as_str())
                                                    .or_else(|| item.get("id").and_then(|v| v.as_str()))
                                                {
                                                    let tool_calls = tool_call_state.finalize_call(
                                                        call_id,
                                                        item.get("name").and_then(|v| v.as_str()),
                                                        item.get("arguments").and_then(|v| v.as_str()),
                                                    );
                                                    for tool_call in tool_calls {
                                                        saw_tool_call = true;
                                                        yield Ok(tool_call_delta(tool_call));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    "response.function_call_arguments.delta" => {
                                        if let Some(call_id) = event.get("call_id")
                                            .and_then(|v| v.as_str())
                                            .or_else(|| event.get("item_id").and_then(|v| v.as_str()))
                                        {
                                            if let Some(delta) = event.get("delta").and_then(|v| v.as_str()) {
                                                tool_call_state.append_arguments_delta(call_id, delta);
                                            }
                                        }
                                    }
                                    "response.function_call_arguments.done" => {
                                        if let Some(call_id) = event.get("call_id")
                                            .and_then(|v| v.as_str())
                                            .or_else(|| event.get("item_id").and_then(|v| v.as_str()))
                                        {
                                            let tool_calls = tool_call_state.finalize_call(
                                                call_id,
                                                event.get("name").and_then(|v| v.as_str()),
                                                event.get("arguments").and_then(|v| v.as_str()),
                                            );
                                            for tool_call in tool_calls {
                                                saw_tool_call = true;
                                                yield Ok(tool_call_delta(tool_call));
                                            }
                                        }
                                    }
                                    "response.output_text.delta" => {
                                        if let Some(delta) = event.get("delta").and_then(|d| d.as_str()) {
                                            saw_text_delta = true;
                                            yield Ok(TextStreamDelta {
                                                text: delta.to_string(),
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
                                    "response.output_text.done" => {
                                        if !saw_text_delta {
                                            if let Some(text) = event.get("text").and_then(|t| t.as_str()) {
                                                if !text.is_empty() {
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
                                        }
                                    }
                                    "response.failed" | "response.error" => {
                                        let message = extract_response_error(&event)
                                            .unwrap_or_else(|| "OpenAI Responses error".to_string());
                                        yield Err(RociError::api(400, message));
                                        saw_done = true;
                                        break;
                                    }
                                    "response.completed" | "response.done" => {
                                        if let Some(message) = extract_response_error(&event) {
                                            yield Err(RociError::api(400, message));
                                            saw_done = true;
                                            break;
                                        }
                                        if let Some(response) = event.get("response") {
                                            if let Some(output) = response.get("output").and_then(|v| v.as_array()) {
                                                if !saw_text_delta {
                                                    let mut completed_text = String::new();
                                                    for item in output {
                                                        if item.get("type").and_then(|t| t.as_str()) == Some("message") {
                                                            if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
                                                                for part in content {
                                                                    if part.get("type").and_then(|t| t.as_str()) == Some("output_text") {
                                                                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                                                            completed_text.push_str(text);
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        } else if item.get("type").and_then(|t| t.as_str()) == Some("output_text") {
                                                            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                                                completed_text.push_str(text);
                                                            }
                                                        }
                                                    }
                                                    if !completed_text.trim().is_empty() {
                                                        saw_text_delta = true;
                                                        yield Ok(TextStreamDelta {
                                                            text: completed_text,
                                                            event_type: StreamEventType::TextDelta,
                                                            tool_call: None,
                                                            finish_reason: None,
                                                            usage: None,
                                                            reasoning: None,
                                                            reasoning_signature: None,
                                                            reasoning_type: None,
                                                        });
                                                    } else if roci_debug_enabled() {
                                                        tracing::debug!("OpenAI Responses completed event had no output text");
                                                    }
                                                }
                                                let tool_calls = tool_call_state.finalize_from_response_output(output);
                                                for tool_call in tool_calls {
                                                    saw_tool_call = true;
                                                    yield Ok(tool_call_delta(tool_call));
                                                }
                                            }
                                        }
                                        let trailing_tool_calls = tool_call_state.flush_ready(true);
                                        for tool_call in trailing_tool_calls {
                                            saw_tool_call = true;
                                            yield Ok(tool_call_delta(tool_call));
                                        }
                                        let finish = event.get("response")
                                            .and_then(|r| r.get("status"))
                                            .and_then(|v| v.as_str())
                                            .and_then(|status| match status {
                                                "completed" => Some(FinishReason::Stop),
                                                "incomplete" => Some(FinishReason::Length),
                                                "failed" => Some(FinishReason::Error),
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
                                            finish_reason: if saw_tool_call {
                                                Some(FinishReason::ToolCalls)
                                            } else {
                                                finish.or(Some(FinishReason::Stop))
                                            },
                                            usage,
                                            reasoning: None,
                                            reasoning_signature: None,
                                            reasoning_type: None,
                                        });
                                    }
                                    _ => {}
                                }
                            }
                            Err(e) => {
                                if roci_debug_enabled() {
                                    tracing::debug!(error = %e, data = %data, "OpenAI Responses SSE parse failed");
                                }
                            }
                        }
                    } else if line.starts_with(':') {
                        continue;
                    } else if let Some(rest) = line.strip_prefix("data:") {
                        let rest = rest.strip_prefix(' ').unwrap_or(rest);
                        pending_data.push(rest.to_string());
                    }
                }

                if saw_done {
                    break;
                }
            }
        };

        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod headers_tests;
#[cfg(test)]
mod response_tests;
#[cfg(test)]
mod tests;
