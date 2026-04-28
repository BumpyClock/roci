//! OpenAI Responses API provider (for GPT-5, o3, o4-mini, etc.)

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

use roci_core::provider::http::{bearer_headers, shared_client};
use roci_core::provider::{ModelProvider, ProviderRequest, ProviderResponse};

use response::ResponsesApiResponse;
use stream::{extract_response_error, tool_call_delta, StreamToolCallState};

use super::openai_errors::status_to_openai_error;

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

    fn resolved_api_key<'a>(&'a self, request: &'a ProviderRequest) -> Result<&'a str, RociError> {
        let default_key = (!self.api_key.is_empty()).then_some(self.api_key.as_str());
        request
            .api_key_override
            .as_deref()
            .or(default_key)
            .ok_or_else(|| RociError::MissingCredential {
                provider: self.provider_name().to_string(),
            })
    }

    fn add_session_affinity_headers(headers: &mut reqwest::header::HeaderMap, session_id: &str) {
        if let Ok(value) = reqwest::header::HeaderValue::from_str(session_id) {
            headers.insert("session_id", value.clone());
            headers.insert("x-client-request-id", value);
        }
    }

    fn build_headers(
        &self,
        request: &ProviderRequest,
    ) -> Result<reqwest::header::HeaderMap, RociError> {
        let resolved_api_key = self.resolved_api_key(request)?;
        let mut headers = bearer_headers(resolved_api_key);
        if self.is_codex {
            let account_id = match (&self.account_id, extract_codex_account_id(resolved_api_key)) {
                (Some(id), _) => Some(id.clone()),
                (None, Ok(id)) => Some(id),
                (None, Err(err)) => {
                    return Err(RociError::Authentication(format!(
                        "Missing Codex account id ({err})"
                    )))
                }
            };
            if let Some(account_id) = account_id {
                if let Ok(value) = reqwest::header::HeaderValue::from_str(&account_id) {
                    headers.insert("chatgpt-account-id", value);
                }
            }
            headers.insert(
                "OpenAI-Beta",
                reqwest::header::HeaderValue::from_static("responses=experimental"),
            );
            headers.insert(
                "originator",
                reqwest::header::HeaderValue::from_static("pi"),
            );
            headers.insert(
                reqwest::header::ACCEPT,
                reqwest::header::HeaderValue::from_static("text/event-stream"),
            );
            let user_agent = format!("roci ({} {})", std::env::consts::OS, std::env::consts::ARCH);
            if let Ok(value) = reqwest::header::HeaderValue::from_str(&user_agent) {
                headers.insert(reqwest::header::USER_AGENT, value);
            }
        } else if let Some(account_id) = &self.account_id {
            if let Ok(value) = reqwest::header::HeaderValue::from_str(account_id) {
                headers.insert("ChatGPT-Account-ID", value);
            }
        }
        if let Some(ref session_id) = request.session_id {
            Self::add_session_affinity_headers(&mut headers, session_id);
        }
        if let Some(ref transport) = request.transport {
            if let Ok(value) = reqwest::header::HeaderValue::from_str(transport) {
                headers.insert("x-roci-transport", value);
            }
        }
        for (name, value) in request.headers.iter() {
            headers.insert(name, value.clone());
        }
        if roci_debug_enabled() {
            tracing::debug!(
                model = self.model.as_str(),
                base_url = %self.base_url,
                account_id_present = self.account_id.is_some(),
                api_key_overridden = request.api_key_override.is_some(),
                request_header_overrides = request.headers.len(),
                codex_headers = self.is_codex,
                "OpenAI Responses headers prepared"
            );
        }
        Ok(headers)
    }
}

fn extract_codex_account_id(token: &str) -> Result<String, RociError> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    let mut parts = token.split('.');
    let _header = parts.next().ok_or_else(|| {
        RociError::Authentication("Invalid Codex token (missing JWT header)".into())
    })?;
    let payload = parts.next().ok_or_else(|| {
        RociError::Authentication("Invalid Codex token (missing JWT payload)".into())
    })?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| RociError::Authentication("Invalid Codex token payload encoding".into()))?;
    let value: serde_json::Value = serde_json::from_slice(&decoded)
        .map_err(|_| RociError::Authentication("Invalid Codex token payload JSON".into()))?;
    let account_id = value
        .get("https://api.openai.com/auth")
        .and_then(|v| v.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| RociError::Authentication("Missing Codex account id claim".into()))?;
    Ok(account_id.to_string())
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

        let status = resp.status().as_u16();
        if status != 200 {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(status_to_openai_error(status, &body_text));
        }

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

        let status = resp.status().as_u16();
        if status != 200 {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(status_to_openai_error(status, &body_text));
        }

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
mod tests {
    use super::response::{
        ResponsesApiResponse, ResponsesChoice, ResponsesChoiceMessage, ResponsesOutputContent,
        ResponsesOutputItem, ResponsesToolCall, ResponsesToolCallFunction,
    };
    use super::*;
    use roci_core::provider::ToolDefinition;

    fn settings() -> GenerationSettings {
        GenerationSettings {
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
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
            google: None,
            tool_choice: None,
            stream_idle_timeout_ms: None,
        }
    }

    #[test]
    fn gpt5_rejects_sampling_settings() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
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
            OpenAiResponsesProvider::new(OpenAiModel::Gpt52, "test-key".to_string(), None, None);
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
            OpenAiResponsesProvider::new(OpenAiModel::Gpt52, "test-key".to_string(), None, None);
        let settings = GenerationSettings {
            temperature: Some(0.7),
            reasoning_effort: Some(ReasoningEffort::None),
            ..Default::default()
        };
        assert!(provider.validate_settings(&settings).is_ok());
    }

    #[test]
    fn gpt41_allows_sampling_settings() {
        let provider = OpenAiResponsesProvider::new(
            OpenAiModel::Gpt41Nano,
            "test-key".to_string(),
            None,
            None,
        );
        let settings = GenerationSettings {
            temperature: Some(0.7),
            top_p: Some(0.9),
            ..Default::default()
        };
        assert!(provider.validate_settings(&settings).is_ok());
    }

    #[test]
    fn gpt41_rejects_text_verbosity_setting() {
        let provider = OpenAiResponsesProvider::new(
            OpenAiModel::Gpt41Nano,
            "test-key".to_string(),
            None,
            None,
        );
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
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
        let settings = GenerationSettings {
            text_verbosity: Some(TextVerbosity::High),
            ..Default::default()
        };
        assert!(provider.validate_settings(&settings).is_ok());
    }

    #[test]
    fn request_body_includes_text_verbosity_and_format() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: GenerationSettings {
                text_verbosity: Some(TextVerbosity::Low),
                ..Default::default()
            },
            tools: None,
            response_format: Some(ResponseFormat::JsonObject),
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: None,
            transport: None,
        };
        let body = provider.build_request_body(&request, false);
        assert_eq!(body["text"]["verbosity"], "low");
        assert_eq!(body["text"]["format"]["type"], "json_object");
    }

    #[test]
    fn request_body_maps_system_to_developer_for_reasoning_models() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
        let request = ProviderRequest {
            messages: vec![
                ModelMessage::system("Use this system message"),
                ModelMessage::user("hello"),
            ],
            settings: settings(),
            tools: None,
            response_format: None,
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: None,
            transport: None,
        };
        let body = provider.build_request_body(&request, false);
        assert_eq!(body["input"][0]["role"], "developer");
        assert_eq!(body["input"][0]["content"], "Use this system message");
    }

    #[test]
    fn request_body_defaults_reasoning_and_text_for_gpt5() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(),
            tools: None,
            response_format: None,
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: None,
            transport: None,
        };
        let body = provider.build_request_body(&request, false);
        assert_eq!(body["reasoning"]["effort"], "medium");
        assert_eq!(body["reasoning"]["summary"], "auto");
        assert_eq!(body["text"]["verbosity"], "high");
        assert!(body.get("truncation").is_none());
    }

    #[test]
    fn request_body_defaults_truncation_for_reasoning_models() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::O3, "test-key".to_string(), None, None);
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(),
            tools: None,
            response_format: None,
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: None,
            transport: None,
        };
        let body = provider.build_request_body(&request, false);
        assert_eq!(body["reasoning"]["effort"], "medium");
        assert_eq!(body["reasoning"]["summary"], "auto");
        assert_eq!(body["truncation"], "auto");
        assert!(body.get("text").is_none());
    }

    #[test]
    fn request_body_includes_openai_responses_options() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("tag".to_string(), "value".to_string());
        let settings = GenerationSettings {
            user: Some("user-1".to_string()),
            openai_responses: Some(OpenAiResponsesOptions {
                parallel_tool_calls: Some(false),
                previous_response_id: Some("resp_1".to_string()),
                instructions: Some("Be brief".to_string()),
                metadata: Some(metadata),
                service_tier: Some(OpenAiServiceTier::Flex),
                truncation: Some(OpenAiTruncation::Auto),
                store: Some(true),
            }),
            ..Default::default()
        };
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings,
            tools: None,
            response_format: None,
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: None,
            transport: None,
        };
        let body = provider.build_request_body(&request, false);
        assert_eq!(body["user"], "user-1");
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(body["previous_response_id"], "resp_1");
        assert_eq!(body["instructions"], "Be brief");
        assert_eq!(body["metadata"]["tag"], "value");
        assert_eq!(body["service_tier"], "flex");
        assert_eq!(body["truncation"], "auto");
        assert_eq!(body["store"], true);
    }

    #[test]
    fn request_body_merges_request_metadata_and_includes_prompt_cache_key() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);

        let mut options_metadata = std::collections::HashMap::new();
        options_metadata.insert("tag".to_string(), "settings".to_string());
        let mut request_metadata = std::collections::HashMap::new();
        request_metadata.insert("tag".to_string(), "request".to_string());
        request_metadata.insert("trace_id".to_string(), "trace-1".to_string());
        let session_id = "session-abc";
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: GenerationSettings {
                openai_responses: Some(OpenAiResponsesOptions {
                    metadata: Some(options_metadata),
                    ..Default::default()
                }),
                ..Default::default()
            },
            tools: None,
            response_format: None,
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: request_metadata,
            payload_callback: None,
            session_id: Some(session_id.to_string()),
            transport: None,
        };

        let body = provider.build_request_body(&request, false);
        assert_eq!(body["prompt_cache_key"], session_id);
        assert!(body.get("previous_response_id").is_none());
        assert_eq!(body["metadata"]["tag"], "request");
        assert_eq!(body["metadata"]["trace_id"], "trace-1");
        assert!(body["metadata"].get("roci_session_id").is_none());
    }

    #[test]
    fn request_body_omits_prompt_cache_key_when_no_session_id() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(),
            tools: None,
            response_format: None,
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: None,
            transport: None,
        };

        let body = provider.build_request_body(&request, false);
        assert!(body.get("prompt_cache_key").is_none());
        assert!(body.get("previous_response_id").is_none());
    }

    #[test]
    fn codex_request_body_includes_prompt_cache_key() {
        let provider = OpenAiResponsesProvider::new(
            OpenAiModel::Gpt5Nano,
            "test-key".to_string(),
            Some("https://chatgpt.com/backend-api/codex".to_string()),
            Some("acct-123".to_string()),
        );
        let session_id = "codex-session-1";
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(),
            tools: None,
            response_format: None,
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: Some(session_id.to_string()),
            transport: None,
        };

        let body = provider.build_request_body(&request, false);
        assert_eq!(body["prompt_cache_key"], session_id);
    }

    #[test]
    fn openai_responses_headers_include_session_affinity() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
        let session_id = "session-1";
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(),
            tools: None,
            response_format: None,
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: Some(session_id.to_string()),
            transport: None,
        };

        let headers = provider.build_headers(&request).expect("headers");
        assert_eq!(
            headers.get("session_id").unwrap().to_str().unwrap(),
            session_id
        );
        assert_eq!(
            headers
                .get("x-client-request-id")
                .unwrap()
                .to_str()
                .unwrap(),
            session_id
        );
    }

    #[test]
    fn openai_responses_headers_omit_session_affinity_when_absent() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(),
            tools: None,
            response_format: None,
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: None,
            transport: None,
        };

        let headers = provider.build_headers(&request).expect("headers");
        assert!(headers.get("session_id").is_none());
        assert!(headers.get("x-client-request-id").is_none());
    }

    #[test]
    fn codex_headers_include_session_affinity() {
        let provider = OpenAiResponsesProvider::new(
            OpenAiModel::Gpt5Nano,
            "test-key".to_string(),
            Some("https://chatgpt.com/backend-api/codex".to_string()),
            Some("acct-123".to_string()),
        );
        let session_id = "codex-session-1";
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(),
            tools: None,
            response_format: None,
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: Some(session_id.to_string()),
            transport: None,
        };

        let headers = provider.build_headers(&request).expect("headers");
        assert_eq!(
            headers.get("session_id").unwrap().to_str().unwrap(),
            session_id
        );
        assert_eq!(
            headers
                .get("x-client-request-id")
                .unwrap()
                .to_str()
                .unwrap(),
            session_id
        );
    }

    #[test]
    fn codex_headers_omit_session_affinity_when_absent() {
        let provider = OpenAiResponsesProvider::new(
            OpenAiModel::Gpt5Nano,
            "test-key".to_string(),
            Some("https://chatgpt.com/backend-api/codex".to_string()),
            Some("acct-123".to_string()),
        );
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(),
            tools: None,
            response_format: None,
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: None,
            transport: None,
        };

        let headers = provider.build_headers(&request).expect("headers");
        assert!(headers.get("session_id").is_none());
        assert!(headers.get("x-client-request-id").is_none());
    }

    #[test]
    fn headers_merge_request_overrides_and_api_key_override() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "base-key".to_string(), None, None);
        let mut request_headers = reqwest::header::HeaderMap::new();
        request_headers.insert(
            "x-request-header",
            reqwest::header::HeaderValue::from_static("value"),
        );
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(),
            tools: None,
            response_format: None,
            api_key_override: Some("override-key".to_string()),
            headers: request_headers,
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: None,
            transport: Some(roci_core::provider::TRANSPORT_PROXY.to_string()),
        };

        let headers = provider
            .build_headers(&request)
            .expect("headers should build");
        assert_eq!(
            headers
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer override-key")
        );
        assert_eq!(
            headers
                .get("x-request-header")
                .and_then(|value| value.to_str().ok()),
            Some("value")
        );
        assert_eq!(
            headers
                .get("x-roci-transport")
                .and_then(|value| value.to_str().ok()),
            Some(roci_core::provider::TRANSPORT_PROXY)
        );
    }

    #[test]
    fn headers_use_request_api_key_override_when_default_key_missing() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, String::new(), None, None);
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(),
            tools: None,
            response_format: None,
            api_key_override: Some("override-key".to_string()),
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: None,
            transport: None,
        };

        let headers = provider
            .build_headers(&request)
            .expect("headers should build");

        assert_eq!(
            headers
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer override-key")
        );
    }

    #[test]
    fn headers_error_when_no_default_or_request_api_key() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, String::new(), None, None);
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(),
            tools: None,
            response_format: None,
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: None,
            transport: None,
        };

        let err = provider.build_headers(&request).unwrap_err();

        assert!(matches!(err, RociError::MissingCredential { .. }));
    }

    #[test]
    fn payload_callback_receives_request_payload() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
        let captured_model = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let captured_model_for_hook = captured_model.clone();
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(),
            tools: None,
            response_format: None,
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: Some(std::sync::Arc::new(move |payload| {
                *captured_model_for_hook.lock().expect("capture lock") = payload
                    .get("model")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
            })),
            session_id: None,
            transport: None,
        };
        let body = provider.build_request_body(&request, false);
        provider.emit_payload_callback(&request, &body);

        assert_eq!(
            captured_model.lock().expect("capture lock").as_deref(),
            Some("gpt-5-nano")
        );
    }

    #[test]
    fn status_error_maps_context_length_to_typed_code() {
        let body = serde_json::json!({
            "error": {
                "message": "This model's maximum context length is 128000 tokens.",
                "type": "invalid_request_error",
                "code": "context_length_exceeded",
                "param": "input"
            }
        })
        .to_string();

        let error = status_to_openai_error(400, &body);
        match error {
            RociError::Api {
                details: Some(details),
                ..
            } => {
                assert_eq!(
                    details.code,
                    Some(roci_core::error::ErrorCode::ContextLengthExceeded)
                );
                assert_eq!(
                    details.provider_code.as_deref(),
                    Some("context_length_exceeded")
                );
                assert_eq!(details.param.as_deref(), Some("input"));
            }
            other => panic!("expected typed API error, got {other:?}"),
        }
    }

    #[test]
    fn tool_parameters_are_normalized_for_responses_api() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: GenerationSettings::default(),
            tools: Some(vec![ToolDefinition {
                name: "get_date".to_string(),
                description: "Return a date".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "format": {"type": "string"}
                    }
                }),
            }]),
            response_format: None,
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: None,
            transport: None,
        };
        let body = provider.build_request_body(&request, false);
        assert_eq!(
            body["tools"][0]["parameters"]["additionalProperties"],
            false
        );
        assert_eq!(
            body["tools"][0]["parameters"]["required"],
            serde_json::json!([])
        );
    }

    #[test]
    fn response_parses_function_call_output_item() {
        let response = ResponsesApiResponse {
            output: Some(vec![ResponsesOutputItem {
                r#type: "function_call".to_string(),
                content: None,
                call_id: Some("call_1".to_string()),
                name: Some("get_date".to_string()),
                arguments: Some(r#"{"date":"today"}"#.to_string()),
                tool_call: None,
            }]),
            choices: None,
            status: Some("completed".to_string()),
            usage: None,
        };

        let parsed = OpenAiResponsesProvider::parse_response(response).unwrap();
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].name, "get_date");
        assert_eq!(parsed.finish_reason, Some(FinishReason::ToolCalls));
    }

    #[test]
    fn response_parses_message_tool_call_content() {
        let tool_call = ResponsesToolCall {
            id: "call_1".to_string(),
            function: ResponsesToolCallFunction {
                name: "get_date".to_string(),
                arguments: r#"{"date":"today"}"#.to_string(),
            },
        };
        let response = ResponsesApiResponse {
            output: Some(vec![ResponsesOutputItem {
                r#type: "message".to_string(),
                content: Some(vec![
                    ResponsesOutputContent {
                        r#type: "output_text".to_string(),
                        text: Some("ok".to_string()),
                        tool_call: None,
                    },
                    ResponsesOutputContent {
                        r#type: "tool_call".to_string(),
                        text: None,
                        tool_call: Some(tool_call),
                    },
                ]),
                call_id: None,
                name: None,
                arguments: None,
                tool_call: None,
            }]),
            choices: None,
            status: Some("completed".to_string()),
            usage: None,
        };

        let parsed = OpenAiResponsesProvider::parse_response(response).unwrap();
        assert_eq!(parsed.text, "ok");
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].name, "get_date");
    }

    #[test]
    fn response_parses_choices_fallback() {
        let tool_call = ResponsesToolCall {
            id: "call_1".to_string(),
            function: ResponsesToolCallFunction {
                name: "get_date".to_string(),
                arguments: r#"{"date":"today"}"#.to_string(),
            },
        };
        let response = ResponsesApiResponse {
            output: None,
            choices: Some(vec![ResponsesChoice {
                message: ResponsesChoiceMessage {
                    content: Some("ok".to_string()),
                    tool_calls: Some(vec![tool_call]),
                },
                finish_reason: Some("stop".to_string()),
            }]),
            status: None,
            usage: None,
        };

        let parsed = OpenAiResponsesProvider::parse_response(response).unwrap();
        assert_eq!(parsed.text, "ok");
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].name, "get_date");
    }

    #[test]
    fn stream_tool_calls_emit_only_after_finalize_events() {
        let mut state = StreamToolCallState::default();

        state.observe_call("call_1", Some("get_date"));
        state.append_arguments_delta("call_1", r#"{"date":"to"#);
        assert!(state.flush_ready(false).is_empty());

        state.append_arguments_delta("call_1", r#"day"}"#);
        let emitted = state.finalize_call("call_1", None, None);
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].id, "call_1");
        assert_eq!(emitted[0].name, "get_date");
        assert_eq!(emitted[0].arguments, serde_json::json!({"date": "today"}));
    }

    #[test]
    fn stream_tool_calls_preserve_order_until_prior_call_finishes() {
        let mut state = StreamToolCallState::default();

        state.observe_call("call_1", Some("first_tool"));
        state.observe_call("call_2", Some("second_tool"));
        assert!(state
            .finalize_call("call_2", None, Some(r#"{"value":2}"#))
            .is_empty());

        let emitted = state.finalize_call("call_1", None, Some(r#"{"value":1}"#));
        assert_eq!(emitted.len(), 2);
        assert_eq!(emitted[0].id, "call_1");
        assert_eq!(emitted[1].id, "call_2");
    }

    #[test]
    fn stream_tool_calls_avoid_duplicates_and_use_response_output_fallback() {
        let mut state = StreamToolCallState::default();

        state.observe_call("call_1", Some("first_tool"));
        let first_emit = state.finalize_call("call_1", None, Some(r#"{"value":1}"#));
        assert_eq!(first_emit.len(), 1);
        assert!(state
            .finalize_call("call_1", Some("first_tool"), Some(r#"{"value":1}"#))
            .is_empty());

        let response_output = vec![
            serde_json::json!({
                "type": "function_call",
                "call_id": "call_1",
                "name": "first_tool",
                "arguments": r#"{"value":1}"#,
            }),
            serde_json::json!({
                "type": "function_call",
                "call_id": "call_2",
                "name": "second_tool",
                "arguments": r#"{"value":2}"#,
            }),
        ];
        let mut emitted = state.finalize_from_response_output(&response_output);
        emitted.extend(state.flush_ready(true));
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].id, "call_2");
        assert_eq!(emitted[0].name, "second_tool");
    }

    #[test]
    fn tool_output_uses_plain_string_content() {
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
        let request = ProviderRequest {
            messages: vec![ModelMessage::tool_result(
                "call_1",
                serde_json::Value::String("ok".to_string()),
                false,
            )],
            settings: settings(),
            tools: None,
            response_format: None,
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: None,
            transport: None,
        };

        let body = provider.build_request_body(&request, false);
        assert_eq!(body["input"][0]["type"], "function_call_output");
        assert_eq!(body["input"][0]["output"], "ok");
    }
}
