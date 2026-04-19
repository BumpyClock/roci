//! Request body building for the OpenAI Responses API.

use std::collections::HashMap;
use std::env;

use roci_core::error::RociError;
use roci_core::types::*;
use roci_core::util::debug::roci_debug_enabled;

use roci_core::provider::format::tool_result_to_string;
use roci_core::provider::{ProviderRequest, TRANSPORT_PROXY};

use super::OpenAiResponsesProvider;

const DEFAULT_CODEX_INSTRUCTIONS: &str = "You are Roci, a helpful assistant.";
const RESPONSES_PROXY_BASE_URL_ENV: &str = "ROCI_OPENAI_RESPONSES_PROXY_BASE_URL";

impl OpenAiResponsesProvider {
    pub(crate) fn validate_settings(&self, settings: &GenerationSettings) -> Result<(), RociError> {
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

    pub(crate) fn resolve_previous_response_id(&self, request: &ProviderRequest) -> Option<String> {
        request
            .settings
            .openai_responses
            .as_ref()
            .and_then(|options| options.previous_response_id.clone())
    }

    pub(crate) fn merged_metadata(
        &self,
        request: &ProviderRequest,
    ) -> Option<HashMap<String, String>> {
        let mut metadata = request
            .settings
            .openai_responses
            .as_ref()
            .and_then(|options| options.metadata.clone())
            .unwrap_or_default();

        for (key, value) in &request.metadata {
            metadata.insert(key.clone(), value.clone());
        }

        if metadata.is_empty() {
            None
        } else {
            Some(metadata)
        }
    }

    pub(crate) fn responses_url(&self, request: &ProviderRequest) -> String {
        let base_url = match request.transport.as_deref() {
            Some(TRANSPORT_PROXY) => env::var(RESPONSES_PROXY_BASE_URL_ENV)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| self.base_url.clone()),
            _ => self.base_url.clone(),
        };
        format!("{}/responses", base_url.trim_end_matches('/'))
    }

    pub(crate) fn emit_payload_callback(
        &self,
        request: &ProviderRequest,
        body: &serde_json::Value,
    ) {
        if let Some(callback) = request.payload_callback.as_ref() {
            callback(body.clone());
        }
    }

    pub(crate) fn build_request_body(
        &self,
        request: &ProviderRequest,
        stream: bool,
    ) -> serde_json::Value {
        if self.is_codex {
            return self.build_codex_request_body(request, stream);
        }
        let system_role = if self.capabilities.supports_reasoning {
            "developer"
        } else {
            "system"
        };
        let input = Self::build_input_items(&request.messages, system_role);

        let mut body = serde_json::json!({
            "model": self.model.as_str(),
            "input": input,
            "stream": stream,
        });

        let obj = body.as_object_mut().unwrap();
        self.insert_generation_controls(obj, request);
        self.insert_tool_definitions(obj, request);

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

        self.insert_request_options(obj, request, true);

        body
    }

    fn build_codex_request_body(
        &self,
        request: &ProviderRequest,
        stream: bool,
    ) -> serde_json::Value {
        let (instructions, filtered_messages, system_count) = Self::extract_codex_instructions(
            &request.messages,
            request
                .settings
                .openai_responses
                .as_ref()
                .and_then(|o| o.instructions.as_ref()),
        );
        let input = Self::build_input_items(&filtered_messages, "system");

        if roci_debug_enabled() {
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
        self.insert_generation_controls(obj, request);
        self.insert_tool_definitions(obj, request);
        self.insert_request_options(obj, request, false);

        body
    }

    fn insert_generation_controls(
        &self,
        obj: &mut serde_json::Map<String, serde_json::Value>,
        request: &ProviderRequest,
    ) {
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
    }

    fn insert_tool_definitions(
        &self,
        obj: &mut serde_json::Map<String, serde_json::Value>,
        request: &ProviderRequest,
    ) {
        if let Some(ref tools) = request.tools {
            if tools.is_empty() {
                return;
            }

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

    fn insert_request_options(
        &self,
        obj: &mut serde_json::Map<String, serde_json::Value>,
        request: &ProviderRequest,
        include_instructions: bool,
    ) {
        if let Some(ref user) = request.settings.user {
            obj.insert("user".into(), user.clone().into());
        }

        if let Some(ref options) = request.settings.openai_responses {
            if let Some(parallel_tool_calls) = options.parallel_tool_calls {
                obj.insert("parallel_tool_calls".into(), parallel_tool_calls.into());
            }
            if include_instructions {
                if let Some(ref instructions) = options.instructions {
                    obj.insert("instructions".into(), instructions.clone().into());
                }
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
        if let Some(previous_response_id) = self.resolve_previous_response_id(request) {
            obj.insert("previous_response_id".into(), previous_response_id.into());
        }
        if let Some(ref session_id) = request.session_id {
            obj.insert("prompt_cache_key".into(), session_id.clone().into());
        }
        if let Some(metadata) = self.merged_metadata(request) {
            obj.insert("metadata".into(), serde_json::json!(metadata));
        }
        if obj.get("truncation").is_none() && self.model.is_reasoning() {
            obj.insert(
                "truncation".into(),
                OpenAiTruncation::Auto.to_string().into(),
            );
        }
    }

    pub(crate) fn build_input_items(
        messages: &[ModelMessage],
        system_role: &str,
    ) -> Vec<serde_json::Value> {
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
                            Role::System => system_role,
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

    pub(crate) fn normalize_tool_parameters(schema: &serde_json::Value) -> serde_json::Value {
        let normalized =
            roci_core::provider::schema::normalize_schema_for_provider(schema, "openai");
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
