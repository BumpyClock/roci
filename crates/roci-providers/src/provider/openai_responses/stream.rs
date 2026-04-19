//! Streaming state machine for OpenAI Responses SSE events.

use roci_core::types::*;

use super::OpenAiResponsesProvider;

/// Build a [`TextStreamDelta`] that carries a completed tool call.
pub(crate) fn tool_call_delta(tool_call: AgentToolCall) -> TextStreamDelta {
    TextStreamDelta {
        text: String::new(),
        event_type: StreamEventType::ToolCallDelta,
        tool_call: Some(tool_call),
        finish_reason: None,
        usage: None,
        reasoning: None,
        reasoning_signature: None,
        reasoning_type: None,
    }
}

/// Tracks in-flight tool calls during a Responses API stream, ensuring
/// calls are emitted in the order they were first observed and only after
/// all argument deltas have been received.
#[derive(Default)]
pub(crate) struct StreamToolCallState {
    call_order: Vec<String>,
    seen_calls: std::collections::HashSet<String>,
    call_names: std::collections::HashMap<String, String>,
    call_arguments: std::collections::HashMap<String, String>,
    ready_calls: std::collections::HashMap<String, AgentToolCall>,
    emitted_calls: std::collections::HashSet<String>,
    next_emit_index: usize,
}

impl StreamToolCallState {
    pub(crate) fn observe_call(&mut self, call_id: &str, name: Option<&str>) {
        if self.seen_calls.insert(call_id.to_string()) {
            self.call_order.push(call_id.to_string());
        }
        if let Some(name) = name {
            self.call_names
                .insert(call_id.to_string(), name.to_string());
        }
    }

    pub(crate) fn append_arguments_delta(&mut self, call_id: &str, delta: &str) {
        self.observe_call(call_id, None);
        self.call_arguments
            .entry(call_id.to_string())
            .or_default()
            .push_str(delta);
    }

    pub(crate) fn finalize_call(
        &mut self,
        call_id: &str,
        name: Option<&str>,
        arguments: Option<&str>,
    ) -> Vec<AgentToolCall> {
        self.observe_call(call_id, name);
        if self.emitted_calls.contains(call_id) {
            return Vec::new();
        }
        if !self.ready_calls.contains_key(call_id) {
            let call_name = name
                .map(|value| value.to_string())
                .or_else(|| self.call_names.get(call_id).cloned());
            let call_arguments = if let Some(arguments) = arguments {
                self.call_arguments.remove(call_id);
                Some(arguments.to_string())
            } else {
                self.call_arguments.remove(call_id)
            };
            if let (Some(call_name), Some(call_arguments)) = (call_name, call_arguments) {
                self.ready_calls.insert(
                    call_id.to_string(),
                    OpenAiResponsesProvider::convert_flat_tool_call(
                        call_id,
                        &call_name,
                        &call_arguments,
                    ),
                );
            }
        }
        self.flush_ready(false)
    }

    pub(crate) fn finalize_from_response_output(
        &mut self,
        output: &[serde_json::Value],
    ) -> Vec<AgentToolCall> {
        let mut emitted = Vec::new();
        for item in output {
            if item.get("type").and_then(|value| value.as_str()) != Some("function_call") {
                continue;
            }
            if let Some(call_id) = item
                .get("call_id")
                .and_then(|value| value.as_str())
                .or_else(|| item.get("id").and_then(|value| value.as_str()))
            {
                emitted.extend(self.finalize_call(
                    call_id,
                    item.get("name").and_then(|value| value.as_str()),
                    item.get("arguments").and_then(|value| value.as_str()),
                ));
            }
        }
        emitted
    }

    pub(crate) fn flush_ready(&mut self, force: bool) -> Vec<AgentToolCall> {
        let mut emitted = Vec::new();
        while self.next_emit_index < self.call_order.len() {
            let call_id = self.call_order[self.next_emit_index].clone();
            if let Some(tool_call) = self.ready_calls.remove(&call_id) {
                self.emitted_calls.insert(call_id);
                self.next_emit_index += 1;
                emitted.push(tool_call);
                continue;
            }
            if self.emitted_calls.contains(&call_id) || force {
                self.next_emit_index += 1;
                continue;
            }
            break;
        }
        emitted
    }
}

/// Extract an error message from a Responses API SSE event payload.
pub(crate) fn extract_response_error(event: &serde_json::Value) -> Option<String> {
    let error = event
        .get("error")
        .or_else(|| event.get("response").and_then(|r| r.get("error")));
    let error = error?;
    if error.is_null() {
        return None;
    }
    if let Some(message) = error.get("message").and_then(|v| v.as_str()) {
        return Some(message.to_string());
    }
    if let Some(detail) = error.get("detail").and_then(|v| v.as_str()) {
        return Some(detail.to_string());
    }
    if let Some(text) = error.as_str() {
        return Some(text.to_string());
    }
    Some(error.to_string())
}
