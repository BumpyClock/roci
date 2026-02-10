//! Provider-specific transcript sanitization.

use std::collections::{HashMap, HashSet};

use crate::types::{ContentPart, ModelMessage, Role};

pub fn sanitize_messages_for_provider(
    messages: &[ModelMessage],
    provider: &str,
) -> Vec<ModelMessage> {
    let mut sanitized: Vec<ModelMessage> = if supports_thinking(provider) {
        messages.to_vec()
    } else {
        messages.iter().filter_map(strip_thinking_blocks).collect()
    };

    if requires_tool_pairing(provider) {
        sanitized = sanitize_tool_result_pairing(&sanitized);
    }

    sanitized
}

fn supports_thinking(provider: &str) -> bool {
    matches!(provider, "anthropic" | "anthropic-compatible")
}

fn requires_tool_pairing(provider: &str) -> bool {
    matches!(provider, "anthropic" | "anthropic-compatible" | "google")
}

fn strip_thinking_blocks(message: &ModelMessage) -> Option<ModelMessage> {
    let mut parts = Vec::new();
    for part in &message.content {
        if matches!(
            part,
            ContentPart::Thinking(_) | ContentPart::RedactedThinking(_)
        ) {
            continue;
        }
        parts.push(part.clone());
    }
    if parts.is_empty() {
        return None;
    }
    let mut next = message.clone();
    next.content = parts;
    Some(next)
}

fn sanitize_tool_result_pairing(messages: &[ModelMessage]) -> Vec<ModelMessage> {
    let mut out: Vec<ModelMessage> = Vec::with_capacity(messages.len());
    let mut seen_tool_results: HashSet<String> = HashSet::new();

    let mut i = 0usize;
    while i < messages.len() {
        let msg = &messages[i];
        if msg.role != Role::Assistant {
            if msg.role != Role::Tool {
                out.push(msg.clone());
            }
            i += 1;
            continue;
        }

        let tool_calls = msg.tool_calls();
        if tool_calls.is_empty() {
            out.push(msg.clone());
            i += 1;
            continue;
        }

        let tool_call_ids: HashSet<String> = tool_calls.iter().map(|tc| tc.id.clone()).collect();
        let mut span_results: HashMap<String, ModelMessage> = HashMap::new();
        let mut remainder: Vec<ModelMessage> = Vec::new();

        let mut j = i + 1;
        for idx in j..messages.len() {
            let next = &messages[idx];
            if matches!(next.role, Role::Assistant | Role::User | Role::System) {
                break;
            }
            if next.role == Role::Tool {
                if let Some(id) = extract_tool_result_id(next) {
                    if tool_call_ids.contains(&id) && !seen_tool_results.contains(&id) {
                        span_results.insert(id.clone(), next.clone());
                        seen_tool_results.insert(id);
                    }
                }
            } else {
                remainder.push(next.clone());
            }
            j += 1;
        }

        out.push(msg.clone());
        for call in tool_calls {
            if let Some(existing) = span_results.get(&call.id) {
                out.push(existing.clone());
            } else {
                out.push(ModelMessage::tool_result(
                    call.id.clone(),
                    serde_json::json!({
                        "error": "missing tool result in transcript; inserted synthetic error result",
                    }),
                    true,
                ));
            }
        }
        out.extend(remainder);
        i = j;
    }

    out
}

fn extract_tool_result_id(message: &ModelMessage) -> Option<String> {
    if message.role != Role::Tool {
        return None;
    }
    for part in &message.content {
        if let ContentPart::ToolResult(result) = part {
            return Some(result.tool_call_id.clone());
        }
    }
    None
}
