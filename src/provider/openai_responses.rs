//! OpenAI Responses API provider (for GPT-5, o3, o4-mini, etc.)

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use serde::Deserialize;
use tracing::debug;
use std::env;

use crate::error::RociError;
use crate::models::capabilities::ModelCapabilities;
use crate::models::openai::OpenAiModel;
use crate::types::*;

use super::format::tool_result_to_string;
use super::http::{bearer_headers, shared_client};
use super::{ModelProvider, ProviderRequest, ProviderResponse};

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_CODEX_INSTRUCTIONS: &str = "You are Homie, a helpful assistant.";

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

    fn build_headers(&self) -> Result<reqwest::header::HeaderMap, RociError> {
        let mut headers = bearer_headers(&self.api_key);
        if self.is_codex {
            let account_id = match (&self.account_id, extract_codex_account_id(&self.api_key)) {
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
        if debug_enabled() {
            tracing::debug!(
                model = self.model.as_str(),
                base_url = %self.base_url,
                account_id_present = self.account_id.is_some(),
                codex_headers = self.is_codex,
                "OpenAI Responses headers prepared"
            );
        }
        Ok(headers)
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
        if self.is_codex {
            return self.build_codex_request_body(request, stream);
        }
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

        let needs_reasoning = self.model.is_reasoning() || self.model.is_gpt5_family();
        if needs_reasoning {
            let effort = request
                .settings
                .reasoning_effort
                .unwrap_or(ReasoningEffort::Medium);
            obj.insert(
                "reasoning".into(),
                serde_json::json!({
                    "effort": effort.to_string(),
                    "summary": "auto",
                }),
            );
        }

        if let Some(ref tools) = request.tools {
            if !tools.is_empty() {
                let tool_defs: Vec<serde_json::Value> = tools
                    .iter()
                    .map(|t| {
                        let parameters = Self::normalize_tool_parameters(&t.parameters);
                        serde_json::json!({
                            "type": "function",
                            "name": t.name,
                            "description": t.description,
                            "parameters": parameters,
                            "strict": false,
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
        let verbosity = request
            .settings
            .text_verbosity
            .or_else(|| self.model.is_gpt5_family().then_some(TextVerbosity::High));
        if let Some(verbosity) = verbosity {
            text_obj.insert("verbosity".into(), verbosity.to_string().into());
        }
        if !text_obj.is_empty() {
            obj.insert("text".into(), serde_json::Value::Object(text_obj));
        }

        if let Some(ref user) = request.settings.user {
            obj.insert("user".into(), user.clone().into());
        }

        if let Some(ref options) = request.settings.openai_responses {
            if let Some(parallel_tool_calls) = options.parallel_tool_calls {
                obj.insert("parallel_tool_calls".into(), parallel_tool_calls.into());
            }
            if let Some(ref previous_response_id) = options.previous_response_id {
                obj.insert(
                    "previous_response_id".into(),
                    previous_response_id.clone().into(),
                );
            }
            if let Some(ref instructions) = options.instructions {
                obj.insert("instructions".into(), instructions.clone().into());
            }
            if let Some(ref metadata) = options.metadata {
                obj.insert("metadata".into(), serde_json::json!(metadata));
            }
            if let Some(service_tier) = options.service_tier {
                obj.insert("service_tier".into(), service_tier.to_string().into());
            }
            if let Some(truncation) = options.truncation {
                obj.insert("truncation".into(), truncation.to_string().into());
            }
            if let Some(store) = options.store {
                obj.insert("store".into(), store.into());
            }
        }
        if obj.get("truncation").is_none() && self.model.is_reasoning() {
            obj.insert(
                "truncation".into(),
                OpenAiTruncation::Auto.to_string().into(),
            );
        }

        body
    }

    fn build_codex_request_body(
        &self,
        request: &ProviderRequest,
        stream: bool,
    ) -> serde_json::Value {
        let (instructions, filtered_messages, system_count) =
            Self::extract_codex_instructions(&request.messages, request.settings.openai_responses.as_ref().and_then(|o| o.instructions.as_ref()));
        let input = Self::build_input_items(&filtered_messages);

        if debug_enabled() {
            tracing::debug!(
                model = self.model.as_str(),
                instructions_len = instructions.len(),
                system_count,
                input_count = input.len(),
                "OpenAI Codex request prepared"
            );
        }

        let mut body = serde_json::json!({
            "model": self.model.as_str(),
            "store": false,
            "stream": stream,
            "instructions": instructions,
            "input": input,
            "text": { "verbosity": request.settings.text_verbosity.unwrap_or(TextVerbosity::Medium).to_string() },
            "include": ["reasoning.encrypted_content"],
            "tool_choice": "auto",
            "parallel_tool_calls": true,
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

        let needs_reasoning = self.model.is_reasoning() || self.model.is_gpt5_family();
        if needs_reasoning {
            let effort = request
                .settings
                .reasoning_effort
                .unwrap_or(ReasoningEffort::Medium);
            obj.insert(
                "reasoning".into(),
                serde_json::json!({
                    "effort": effort.to_string(),
                    "summary": "auto",
                }),
            );
        }

        if let Some(ref tools) = request.tools {
            if !tools.is_empty() {
                let tool_defs: Vec<serde_json::Value> = tools
                    .iter()
                    .map(|t| {
                        let parameters = Self::normalize_tool_parameters(&t.parameters);
                        serde_json::json!({
                            "type": "function",
                            "name": t.name,
                            "description": t.description,
                            "parameters": parameters,
                            "strict": false,
                        })
                    })
                    .collect();
                obj.insert("tools".into(), tool_defs.into());
            }
        }

        if let Some(ref user) = request.settings.user {
            obj.insert("user".into(), user.clone().into());
        }

        if let Some(ref options) = request.settings.openai_responses {
            if let Some(parallel_tool_calls) = options.parallel_tool_calls {
                obj.insert("parallel_tool_calls".into(), parallel_tool_calls.into());
            }
            if let Some(ref previous_response_id) = options.previous_response_id {
                obj.insert(
                    "previous_response_id".into(),
                    previous_response_id.clone().into(),
                );
            }
            if let Some(ref metadata) = options.metadata {
                obj.insert("metadata".into(), serde_json::json!(metadata));
            }
            if let Some(service_tier) = options.service_tier {
                obj.insert("service_tier".into(), service_tier.to_string().into());
            }
            if let Some(truncation) = options.truncation {
                obj.insert("truncation".into(), truncation.to_string().into());
            }
            if let Some(store) = options.store {
                obj.insert("store".into(), store.into());
            }
        }
        if obj.get("truncation").is_none() && self.model.is_reasoning() {
            obj.insert(
                "truncation".into(),
                OpenAiTruncation::Auto.to_string().into(),
            );
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
                    ContentPart::Thinking(_) => {}
                    ContentPart::RedactedThinking(_) => {}
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
                                "output": tool_result_to_string(&tr.result),
                            }));
                        }
                    }
                }
            }
        }
        input
    }

    fn extract_codex_instructions(
        messages: &[ModelMessage],
        override_instructions: Option<&String>,
    ) -> (String, Vec<ModelMessage>, usize) {
        let mut system_parts: Vec<String> = Vec::new();
        let mut filtered: Vec<ModelMessage> = Vec::with_capacity(messages.len());
        let mut system_count = 0usize;

        for msg in messages {
            if msg.role == Role::System {
                system_count += 1;
                for part in &msg.content {
                    if let ContentPart::Text { text } = part {
                        if !text.trim().is_empty() {
                            system_parts.push(text.trim().to_string());
                        }
                    }
                }
            } else {
                filtered.push(msg.clone());
            }
        }

        let mut instructions = override_instructions
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| system_parts.join("\n\n").trim().to_string());

        if instructions.is_empty() {
            instructions = DEFAULT_CODEX_INSTRUCTIONS.to_string();
        }

        (instructions, filtered, system_count)
    }

    /// Convert a Responses API payload into a provider response.
    fn parse_response(data: ResponsesApiResponse) -> Result<ProviderResponse, RociError> {
        if let Some(outputs) = data.output {
            let mut text = String::new();
            let mut tool_calls = Vec::new();

            for output in outputs {
                match output.r#type.as_str() {
                    "message" => {
                        if let Some(content) = output.content {
                            for chunk in content {
                                match chunk.r#type.as_str() {
                                    "output_text" => {
                                        if let Some(segment) = chunk.text {
                                            text.push_str(&segment);
                                        }
                                    }
                                    "tool_call" => {
                                        if let Some(tool_call) = chunk.tool_call {
                                            tool_calls.push(Self::convert_tool_call(tool_call));
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    "function_call" => {
                        if let (Some(id), Some(name), Some(args)) =
                            (output.call_id, output.name, output.arguments)
                        {
                            tool_calls.push(Self::convert_flat_tool_call(&id, &name, &args));
                        }
                    }
                    "tool_call" => {
                        if let Some(tool_call) = output.tool_call {
                            tool_calls.push(Self::convert_tool_call(tool_call));
                        }
                    }
                    _ => {}
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

            return Ok(ProviderResponse {
                text,
                usage: Self::map_usage(data.usage),
                tool_calls,
                finish_reason,
                thinking: Vec::new(),
            });
        }

        if let Some(choices) = data.choices {
            let choice = choices
                .into_iter()
                .next()
                .ok_or_else(|| RociError::api(200, "No choices in OpenAI response"))?;
            let tool_calls = choice
                .message
                .tool_calls
                .unwrap_or_default()
                .into_iter()
                .map(Self::convert_tool_call)
                .collect::<Vec<_>>();
            let finish_reason = choice
                .finish_reason
                .as_deref()
                .and_then(|reason| match reason {
                    "stop" => Some(FinishReason::Stop),
                    "length" => Some(FinishReason::Length),
                    "tool_calls" => Some(FinishReason::ToolCalls),
                    _ => None,
                });
            let finish_reason = if !tool_calls.is_empty() {
                Some(FinishReason::ToolCalls)
            } else {
                finish_reason
            };

            return Ok(ProviderResponse {
                text: choice.message.content.unwrap_or_default(),
                usage: Self::map_usage(data.usage),
                tool_calls,
                finish_reason,
                thinking: Vec::new(),
            });
        }

        Err(RociError::api(
            200,
            "No output or choices in OpenAI response",
        ))
    }

    fn convert_flat_tool_call(id: &str, name: &str, args: &str) -> AgentToolCall {
        AgentToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: serde_json::from_str(args)
                .unwrap_or(serde_json::Value::String(args.to_string())),
            recipient: None,
        }
    }

    fn convert_tool_call(tool_call: ResponsesToolCall) -> AgentToolCall {
        Self::convert_flat_tool_call(
            &tool_call.id,
            &tool_call.function.name,
            &tool_call.function.arguments,
        )
    }

    fn map_usage(usage: Option<ResponsesUsage>) -> Usage {
        usage
            .map(|u| {
                let input_tokens = u.input_tokens.or(u.prompt_tokens).unwrap_or(0);
                let output_tokens = u.output_tokens.or(u.completion_tokens).unwrap_or(0);
                let total_tokens = u.total_tokens.unwrap_or(input_tokens + output_tokens);
                Usage {
                    input_tokens,
                    output_tokens,
                    total_tokens,
                    ..Default::default()
                }
            })
            .unwrap_or_default()
    }

    fn normalize_tool_parameters(schema: &serde_json::Value) -> serde_json::Value {
        let normalized = crate::provider::schema::normalize_schema_for_provider(schema, "openai");
        if let Some(obj) = normalized.as_object() {
            let mut next = obj.clone();
            if matches!(next.get("type"), Some(serde_json::Value::String(t)) if t == "object") {
                next.entry("required")
                    .or_insert_with(|| serde_json::Value::Array(Vec::new()));
            }
            serde_json::Value::Object(next)
        } else {
            normalized
        }
    }
}

fn debug_enabled() -> bool {
    matches!(env::var("HOMIE_DEBUG").as_deref(), Ok("1" | "true" | "TRUE"))
        || matches!(env::var("HOME_DEBUG").as_deref(), Ok("1" | "true" | "TRUE"))
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
    let value: serde_json::Value = serde_json::from_slice(&decoded).map_err(|_| {
        RociError::Authentication("Invalid Codex token payload JSON".into())
    })?;
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
        let url = format!("{}/responses", self.base_url);

        debug!(
            model = self.model.as_str(),
            "OpenAI Responses generate_text"
        );

        let resp = shared_client()
            .post(&url)
            .headers(self.build_headers()?)
            .json(&body)
            .send()
            .await?;

        let status = resp.status().as_u16();
        if status != 200 {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(super::http::status_to_error(status, &body_text));
        }

        let data: ResponsesApiResponse = resp.json().await?;
        Self::parse_response(data)
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
            .headers(self.build_headers()?)
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
            let mut call_arguments: std::collections::HashMap<String, String> = std::collections::HashMap::new();
            let mut emitted_calls: std::collections::HashSet<String> = std::collections::HashSet::new();
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
                        if debug_enabled() && debug_event_count < 5 {
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
                                                if let (Some(id), Some(name)) = (
                                                    item.get("call_id").and_then(|v| v.as_str()).or_else(|| item.get("id").and_then(|v| v.as_str())),
                                                    item.get("name").and_then(|v| v.as_str()),
                                                ) {
                                                    call_names.insert(id.to_string(), name.to_string());
                                                    if let Some(args) = item.get("arguments").and_then(|v| v.as_str()) {
                                                        if !args.trim().is_empty() && !emitted_calls.contains(id) {
                                                            let arguments = serde_json::from_str(args)
                                                                .unwrap_or(serde_json::Value::String(args.to_string()));
                                                            yield Ok(TextStreamDelta {
                                                                text: String::new(),
                                                                event_type: StreamEventType::ToolCallDelta,
                                                                tool_call: Some(AgentToolCall {
                                                                    id: id.to_string(),
                                                                    name: name.to_string(),
                                                                    arguments,
                                                                    recipient: None,
                                                                }),
                                                                finish_reason: None,
                                                                usage: None,
                                                                reasoning: None,
                                                                reasoning_signature: None,
                                                                reasoning_type: None,
                                                            });
                                                            emitted_calls.insert(id.to_string());
                                                            saw_tool_call = true;
                                                        }
                                                    }
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
                                                let call_id = item
                                                    .get("call_id")
                                                    .and_then(|v| v.as_str())
                                                    .or_else(|| item.get("id").and_then(|v| v.as_str()))
                                                    .map(|v| v.to_string());
                                                let name = item.get("name").and_then(|v| v.as_str()).map(|v| v.to_string());
                                                let args = item
                                                    .get("arguments")
                                                    .and_then(|v| v.as_str())
                                                    .map(|v| v.to_string())
                                                    .or_else(|| {
                                                        call_id
                                                            .as_ref()
                                                            .and_then(|id| call_arguments.remove(id))
                                                    });
                                                if let (Some(id), Some(name), Some(args)) = (call_id, name, args) {
                                                    if !emitted_calls.contains(&id) {
                                                        let arguments = serde_json::from_str(&args)
                                                            .unwrap_or(serde_json::Value::String(args.to_string()));
                                                        yield Ok(TextStreamDelta {
                                                            text: String::new(),
                                                            event_type: StreamEventType::ToolCallDelta,
                                                            tool_call: Some(AgentToolCall { id: id.clone(), name, arguments, recipient: None }),
                                                            finish_reason: None,
                                                            usage: None,
                                                            reasoning: None,
                                                            reasoning_signature: None,
                                                            reasoning_type: None,
                                                        });
                                                        emitted_calls.insert(id);
                                                        saw_tool_call = true;
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
                                                call_arguments
                                                    .entry(call_id.to_string())
                                                    .or_default()
                                                    .push_str(delta);
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
                                        let args = event
                                            .get("arguments")
                                            .and_then(|v| v.as_str())
                                            .map(|v| v.to_string())
                                            .or_else(|| {
                                                call_id
                                                    .as_ref()
                                                    .and_then(|id| call_arguments.remove(id))
                                            });
                                        if let (Some(id), Some(name), Some(args)) = (call_id, name, args) {
                                            if !emitted_calls.contains(&id) {
                                                let arguments = serde_json::from_str(&args)
                                                    .unwrap_or(serde_json::Value::String(args.to_string()));
                                                yield Ok(TextStreamDelta {
                                                    text: String::new(),
                                                    event_type: StreamEventType::ToolCallDelta,
                                                    tool_call: Some(AgentToolCall { id: id.clone(), name, arguments, recipient: None }),
                                                    finish_reason: None,
                                                    usage: None,
                                                    reasoning: None,
                                                    reasoning_signature: None,
                                                    reasoning_type: None,
                                                });
                                                emitted_calls.insert(id);
                                                saw_tool_call = true;
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
                                    "response.completed" | "response.done" => {
                                        if !saw_text_delta {
                                            if let Some(response) = event.get("response") {
                                                if let Some(output) = response.get("output").and_then(|v| v.as_array()) {
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
                                                    } else if debug_enabled() {
                                                        tracing::debug!("OpenAI Responses completed event had no output text");
                                                    }
                                                }
                                            }
                                        }
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
                                if debug_enabled() {
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

// Internal response types for Responses API

#[derive(Deserialize)]
struct ResponsesApiResponse {
    output: Option<Vec<ResponsesOutputItem>>,
    choices: Option<Vec<ResponsesChoice>>,
    status: Option<String>,
    usage: Option<ResponsesUsage>,
}

#[derive(Deserialize)]
struct ResponsesOutputItem {
    r#type: String,
    content: Option<Vec<ResponsesOutputContent>>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
    #[serde(default)]
    tool_call: Option<ResponsesToolCall>,
}

#[derive(Deserialize)]
struct ResponsesOutputContent {
    r#type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    tool_call: Option<ResponsesToolCall>,
}

#[derive(Deserialize)]
struct ResponsesChoice {
    message: ResponsesChoiceMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponsesChoiceMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ResponsesToolCall>>,
}

#[derive(Deserialize)]
struct ResponsesToolCall {
    id: String,
    function: ResponsesToolCallFunction,
}

#[derive(Deserialize)]
struct ResponsesToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct ResponsesUsage {
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: Option<u32>,
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
    #[serde(default)]
    total_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ToolDefinition;

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
        let provider =
            OpenAiResponsesProvider::new(OpenAiModel::Gpt41Nano, "test-key".to_string(), None, None);
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
            OpenAiResponsesProvider::new(OpenAiModel::Gpt41Nano, "test-key".to_string(), None, None);
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
        };
        let body = provider.build_request_body(&request, false);
        assert_eq!(body["text"]["verbosity"], "low");
        assert_eq!(body["text"]["format"]["type"], "json_object");
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
        };
        let body = provider.build_request_body(&request, false);
        assert_eq!(body["reasoning"]["effort"], "medium");
        assert_eq!(body["reasoning"]["summary"], "auto");
        assert_eq!(body["text"]["verbosity"], "high");
        assert!(body.get("truncation").is_none());
    }

    #[test]
    fn request_body_defaults_truncation_for_reasoning_models() {
        let provider = OpenAiResponsesProvider::new(OpenAiModel::O3, "test-key".to_string(), None, None);
        let request = ProviderRequest {
            messages: vec![ModelMessage::user("hello")],
            settings: settings(),
            tools: None,
            response_format: None,
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
        };

        let body = provider.build_request_body(&request, false);
        assert_eq!(body["input"][0]["type"], "function_call_output");
        assert_eq!(body["input"][0]["output"], "ok");
    }
}
