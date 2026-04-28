//! OpenAI Chat Completions API provider.

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use tracing::debug;

use roci_core::error::RociError;
use roci_core::models::capabilities::ModelCapabilities;
use roci_core::types::*;

use roci_core::provider::format::tool_result_to_string;
use roci_core::provider::http::{bearer_headers, shared_client};
use roci_core::provider::{ModelProvider, ProviderRequest, ProviderResponse};

use super::openai_errors::status_to_openai_error;
use crate::models::openai::OpenAiModel;
use roci_core::util::debug::roci_debug_enabled;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

#[derive(Clone, Copy)]
enum AuthMode {
    Bearer,
    #[cfg(feature = "azure")]
    ApiKey,
}

pub struct OpenAiProvider {
    model: OpenAiModel,
    api_key: String,
    base_url: String,
    account_id: Option<String>,
    extra_headers: reqwest::header::HeaderMap,
    extra_query: Option<String>,
    auth_mode: AuthMode,
    auth_required: bool,
    capabilities: ModelCapabilities,
}

impl OpenAiProvider {
    pub fn new(
        model: OpenAiModel,
        api_key: String,
        base_url: Option<String>,
        account_id: Option<String>,
    ) -> Self {
        Self::new_with_extra_headers(
            model,
            api_key,
            base_url,
            account_id,
            reqwest::header::HeaderMap::new(),
        )
    }

    pub fn new_with_extra_headers(
        model: OpenAiModel,
        api_key: String,
        base_url: Option<String>,
        account_id: Option<String>,
        extra_headers: reqwest::header::HeaderMap,
    ) -> Self {
        Self::new_full(model, api_key, base_url, account_id, extra_headers, None)
    }

    /// Full constructor with all options including extra query parameters.
    pub fn new_full(
        model: OpenAiModel,
        api_key: String,
        base_url: Option<String>,
        account_id: Option<String>,
        extra_headers: reqwest::header::HeaderMap,
        extra_query: Option<String>,
    ) -> Self {
        Self::new_full_with_auth(
            model,
            api_key,
            base_url,
            account_id,
            extra_headers,
            extra_query,
            AuthMode::Bearer,
        )
    }

    #[cfg(feature = "azure")]
    pub(crate) fn new_with_api_key_auth(
        model: OpenAiModel,
        api_key: String,
        base_url: Option<String>,
        account_id: Option<String>,
        extra_headers: reqwest::header::HeaderMap,
        extra_query: Option<String>,
    ) -> Self {
        Self::new_full_with_auth(
            model,
            api_key,
            base_url,
            account_id,
            extra_headers,
            extra_query,
            AuthMode::ApiKey,
        )
    }

    fn new_full_with_auth(
        model: OpenAiModel,
        api_key: String,
        base_url: Option<String>,
        account_id: Option<String>,
        extra_headers: reqwest::header::HeaderMap,
        extra_query: Option<String>,
        auth_mode: AuthMode,
    ) -> Self {
        Self::new_full_with_auth_required(
            model,
            api_key,
            base_url,
            account_id,
            extra_headers,
            extra_query,
            auth_mode,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_full_with_auth_required(
        model: OpenAiModel,
        api_key: String,
        base_url: Option<String>,
        account_id: Option<String>,
        extra_headers: reqwest::header::HeaderMap,
        extra_query: Option<String>,
        auth_mode: AuthMode,
        auth_required: bool,
    ) -> Self {
        let capabilities = model.capabilities();
        Self {
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            model,
            api_key,
            account_id,
            extra_headers,
            extra_query,
            auth_mode,
            auth_required,
            capabilities,
        }
    }

    #[cfg_attr(
        not(any(feature = "lmstudio", feature = "ollama", test)),
        allow(dead_code)
    )]
    pub(crate) fn new_without_auth(model: OpenAiModel, base_url: Option<String>) -> Self {
        Self::new_full_with_auth_required(
            model,
            String::new(),
            base_url,
            None,
            HeaderMap::new(),
            None,
            AuthMode::Bearer,
            false,
        )
    }

    fn resolved_api_key<'a>(
        &'a self,
        request: &'a ProviderRequest,
    ) -> Result<Option<&'a str>, RociError> {
        if let Some(api_key) = request
            .api_key_override
            .as_deref()
            .filter(|api_key| !api_key.is_empty())
        {
            return Ok(Some(api_key));
        }
        if !self.api_key.is_empty() {
            return Ok(Some(self.api_key.as_str()));
        }
        if self.auth_required {
            return Err(RociError::MissingCredential {
                provider: self.provider_name().to_string(),
            });
        }
        Ok(None)
    }

    pub(crate) fn build_headers(
        &self,
        request: &ProviderRequest,
    ) -> Result<reqwest::header::HeaderMap, RociError> {
        let resolved_api_key = self.resolved_api_key(request)?;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        if let Some(resolved_api_key) = resolved_api_key {
            match self.auth_mode {
                AuthMode::Bearer => {
                    if let Some(value) =
                        bearer_headers(resolved_api_key).get(AUTHORIZATION).cloned()
                    {
                        headers.insert(AUTHORIZATION, value);
                    }
                }
                #[cfg(feature = "azure")]
                AuthMode::ApiKey => {
                    if let Ok(value) = HeaderValue::from_str(resolved_api_key) {
                        headers.insert("api-key", value);
                    }
                }
            }
        }

        if let Some(account_id) = &self.account_id {
            if let Ok(value) = reqwest::header::HeaderValue::from_str(account_id) {
                headers.insert("ChatGPT-Account-ID", value);
            }
        }
        for (name, value) in self.extra_headers.iter() {
            headers.insert(name, value.clone());
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
                "OpenAI Chat headers prepared"
            );
        }
        Ok(headers)
    }

    /// Build the chat completions URL, appending extra query params if present.
    pub(crate) fn chat_url(&self) -> String {
        let path = format!("{}/chat/completions", self.base_url);
        match &self.extra_query {
            Some(q) => format!("{}?{}", path, q),
            None => path,
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
                                "strict": false,
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
        let url = self.chat_url();

        debug!(model = self.model.as_str(), "OpenAI generate_text");

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
        let url = self.chat_url();

        debug!(model = self.model.as_str(), "OpenAI stream_text");

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
                if roci_debug_enabled() && chunk_count == 1 {
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
                        if roci_debug_enabled() {
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

                    if let Some(data) = roci_core::provider::http::parse_sse_data(&line) {
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
                        } else if roci_debug_enabled() {
                            debug!(line_len = line.len(), "OpenAI stream parse failed");
                        }
                    }
                }
            }

            if roci_debug_enabled() {
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
            ContentPart::ToolResult(_) => None,
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
            stream_idle_timeout_ms: None,
        }
    }

    fn request_with_headers(
        api_key_override: Option<&str>,
        headers: reqwest::header::HeaderMap,
    ) -> ProviderRequest {
        ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: GenerationSettings::default(),
            tools: None,
            response_format: None,
            api_key_override: api_key_override.map(str::to_string),
            headers,
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: None,
            transport: None,
        }
    }

    #[test]
    fn chat_request_uses_max_completion_tokens_for_gpt5() {
        let provider = OpenAiProvider::new(
            OpenAiModel::Custom("gpt-5-nano".to_string()),
            "test-key".to_string(),
            None,
            None,
        );
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(Some(128), Some(0.4), Some(0.5), Some(0.1), Some(0.2)),
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
        let provider = OpenAiProvider::new(OpenAiModel::Gpt4o, "test-key".to_string(), None, None);
        let message =
            ModelMessage::tool_result("call_1", serde_json::Value::String("ok".to_string()), false);
        let request = ProviderRequest {
            messages: vec![message],
            settings: settings(None, None, None, None, None),
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
        assert_eq!(body["messages"][0]["content"], "ok");
    }

    #[test]
    fn chat_url_without_extra_query() {
        let provider = OpenAiProvider::new(OpenAiModel::Gpt4o, "k".to_string(), None, None);
        assert_eq!(
            provider.chat_url(),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn chat_url_with_extra_query() {
        let provider = OpenAiProvider::new_full(
            OpenAiModel::Gpt4o,
            "k".to_string(),
            Some("https://example.com/v1".to_string()),
            None,
            reqwest::header::HeaderMap::new(),
            Some("api-version=2024-06-01".to_string()),
        );
        assert_eq!(
            provider.chat_url(),
            "https://example.com/v1/chat/completions?api-version=2024-06-01"
        );
    }

    #[cfg(feature = "azure")]
    #[test]
    fn api_key_auth_uses_api_key_header_without_authorization() {
        let provider = OpenAiProvider::new_with_api_key_auth(
            OpenAiModel::Gpt4o,
            "k".to_string(),
            Some("https://example.com/v1".to_string()),
            None,
            HeaderMap::new(),
            Some("api-version=2024-06-01".to_string()),
        );
        let request = request_with_headers(None, HeaderMap::new());
        let headers = provider.build_headers(&request).expect("headers");
        assert_eq!(
            headers.get("api-key").and_then(|value| value.to_str().ok()),
            Some("k")
        );
        assert!(headers.get(AUTHORIZATION).is_none());
    }

    #[test]
    fn headers_use_request_api_key_override_when_default_key_missing() {
        let provider = OpenAiProvider::new(OpenAiModel::Gpt4o, String::new(), None, None);
        let request = request_with_headers(Some("request-key"), HeaderMap::new());

        let headers = provider.build_headers(&request).expect("headers");

        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer request-key")
        );
    }

    #[test]
    fn headers_treat_empty_request_api_key_override_as_missing() {
        let provider =
            OpenAiProvider::new(OpenAiModel::Gpt4o, "default-key".to_string(), None, None);
        let request = request_with_headers(Some(""), HeaderMap::new());

        let headers = provider.build_headers(&request).expect("headers");

        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer default-key")
        );
    }

    #[test]
    fn headers_error_when_empty_request_api_key_override_and_no_default_key() {
        let provider = OpenAiProvider::new(OpenAiModel::Gpt4o, String::new(), None, None);
        let request = request_with_headers(Some(""), HeaderMap::new());

        let err = provider.build_headers(&request).unwrap_err();

        assert!(matches!(err, RociError::MissingCredential { .. }));
    }

    #[test]
    fn headers_merge_request_overrides_after_auth_and_extra_headers() {
        let mut extra = HeaderMap::new();
        extra.insert("x-extra", HeaderValue::from_static("base"));
        let provider = OpenAiProvider::new_with_extra_headers(
            OpenAiModel::Gpt4o,
            "default-key".to_string(),
            None,
            None,
            extra,
        );
        let mut request_headers = HeaderMap::new();
        request_headers.insert("x-extra", HeaderValue::from_static("request"));
        request_headers.insert("x-request", HeaderValue::from_static("value"));
        let request = request_with_headers(Some("request-key"), request_headers);

        let headers = provider.build_headers(&request).expect("headers");

        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer request-key")
        );
        assert_eq!(
            headers.get("x-extra").and_then(|value| value.to_str().ok()),
            Some("request")
        );
        assert_eq!(
            headers
                .get("x-request")
                .and_then(|value| value.to_str().ok()),
            Some("value")
        );
    }

    #[test]
    fn headers_error_when_no_default_or_request_api_key() {
        let provider = OpenAiProvider::new(OpenAiModel::Gpt4o, String::new(), None, None);
        let request = request_with_headers(None, HeaderMap::new());

        let err = provider.build_headers(&request).unwrap_err();

        assert!(matches!(err, RociError::MissingCredential { .. }));
    }

    #[test]
    fn headers_omit_authorization_when_auth_not_required() {
        let provider = OpenAiProvider::new_without_auth(OpenAiModel::Gpt4o, None);
        let request = request_with_headers(None, HeaderMap::new());

        let headers = provider.build_headers(&request).expect("headers");

        assert!(headers.get(AUTHORIZATION).is_none());
        assert_eq!(
            headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
    }
}
