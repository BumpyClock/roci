use crate::types::{message::ContentPart, AgentToolCall, ModelMessage};

use super::control::AgentEventEmitter;
use super::AgentEvent;

fn build_assistant_message(iteration_text: &str, tool_calls: &[AgentToolCall]) -> ModelMessage {
    let mut content: Vec<ContentPart> = Vec::new();
    if !iteration_text.is_empty() {
        content.push(ContentPart::Text {
            text: iteration_text.to_string(),
        });
    }
    for call in tool_calls {
        content.push(ContentPart::ToolCall(call.clone()));
    }
    if content.is_empty() {
        content.push(ContentPart::Text {
            text: String::new(),
        });
    }
    ModelMessage {
        role: crate::types::Role::Assistant,
        content,
        name: None,
        timestamp: Some(chrono::Utc::now()),
    }
}

pub(super) fn emit_message_start_if_needed(
    agent_emitter: &AgentEventEmitter,
    message_open: &mut bool,
    iteration_text: &str,
    tool_calls: &[AgentToolCall],
) {
    if !*message_open {
        agent_emitter.emit(AgentEvent::MessageStart {
            message: build_assistant_message(iteration_text, tool_calls),
        });
        *message_open = true;
    }
}

pub(super) fn emit_message_end_if_open(
    agent_emitter: &AgentEventEmitter,
    message_open: &mut bool,
    iteration_text: &str,
    tool_calls: &[AgentToolCall],
) {
    if *message_open {
        agent_emitter.emit(AgentEvent::MessageEnd {
            message: build_assistant_message(iteration_text, tool_calls),
        });
        *message_open = false;
    }
}

pub(super) fn emit_message_lifecycle(agent_emitter: &AgentEventEmitter, message: &ModelMessage) {
    agent_emitter.emit(AgentEvent::MessageStart {
        message: message.clone(),
    });
    agent_emitter.emit(AgentEvent::MessageEnd {
        message: message.clone(),
    });
}

pub(super) fn assistant_message_snapshot(
    iteration_text: &str,
    tool_calls: &[AgentToolCall],
) -> ModelMessage {
    build_assistant_message(iteration_text, tool_calls)
}
