use super::super::ProviderScenario;
use crate::error::RociError;
use crate::types::{AgentToolCall, StreamEventType, TextStreamDelta, Usage};

pub(super) fn events_for_scenario(
    scenario: ProviderScenario,
    call_index: usize,
) -> Result<Vec<Result<TextStreamDelta, RociError>>, RociError> {
    match scenario {
        ProviderScenario::ParallelSafeBatchThenComplete => {
            if call_index == 0 {
                Ok(vec![
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: Some(AgentToolCall {
                            id: "safe-read-1".to_string(),
                            name: "read".to_string(),
                            arguments: serde_json::json!({}),
                            recipient: None,
                        }),
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: Some(AgentToolCall {
                            id: "safe-ls-2".to_string(),
                            name: "ls".to_string(),
                            arguments: serde_json::json!({}),
                            recipient: None,
                        }),
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Done,
                        tool_call: None,
                        finish_reason: None,
                        usage: Some(Usage::default()),
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                ])
            } else {
                Ok(vec![Ok(TextStreamDelta {
                    text: String::new(),
                    event_type: StreamEventType::Done,
                    tool_call: None,
                    finish_reason: None,
                    usage: Some(Usage::default()),
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                })])
            }
        }
        ProviderScenario::MutatingBatchThenComplete => {
            if call_index == 0 {
                Ok(vec![
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: Some(AgentToolCall {
                            id: "mutating-call-1".to_string(),
                            name: "apply_patch".to_string(),
                            arguments: serde_json::json!({}),
                            recipient: None,
                        }),
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: Some(AgentToolCall {
                            id: "safe-read-2".to_string(),
                            name: "read".to_string(),
                            arguments: serde_json::json!({}),
                            recipient: None,
                        }),
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Done,
                        tool_call: None,
                        finish_reason: None,
                        usage: Some(Usage::default()),
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                ])
            } else {
                Ok(vec![Ok(TextStreamDelta {
                    text: String::new(),
                    event_type: StreamEventType::Done,
                    tool_call: None,
                    finish_reason: None,
                    usage: Some(Usage::default()),
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                })])
            }
        }
        ProviderScenario::MixedTextAndParallelBatchThenComplete => {
            if call_index == 0 {
                Ok(vec![
                    Ok(TextStreamDelta {
                        text: "Gathering context.".to_string(),
                        event_type: StreamEventType::TextDelta,
                        tool_call: None,
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: Some(AgentToolCall {
                            id: "mixed-read-1".to_string(),
                            name: "read".to_string(),
                            arguments: serde_json::json!({ "path": "README.md" }),
                            recipient: None,
                        }),
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: Some(AgentToolCall {
                            id: "mixed-ls-2".to_string(),
                            name: "ls".to_string(),
                            arguments: serde_json::json!({ "path": "." }),
                            recipient: None,
                        }),
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Done,
                        tool_call: None,
                        finish_reason: None,
                        usage: Some(Usage::default()),
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                ])
            } else {
                Ok(vec![
                    Ok(TextStreamDelta {
                        text: "complete".to_string(),
                        event_type: StreamEventType::TextDelta,
                        tool_call: None,
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Done,
                        tool_call: None,
                        finish_reason: None,
                        usage: Some(Usage::default()),
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                ])
            }
        }
        ProviderScenario::DuplicateToolCallDeltaThenComplete => {
            if call_index == 0 {
                Ok(vec![
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: Some(AgentToolCall {
                            id: "dup-read-1".to_string(),
                            name: "read".to_string(),
                            arguments: serde_json::json!({ "path": "first" }),
                            recipient: None,
                        }),
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: Some(AgentToolCall {
                            id: "dup-read-1".to_string(),
                            name: "read".to_string(),
                            arguments: serde_json::json!({ "path": "second" }),
                            recipient: None,
                        }),
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Done,
                        tool_call: None,
                        finish_reason: None,
                        usage: Some(Usage::default()),
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                ])
            } else {
                Ok(vec![Ok(TextStreamDelta {
                    text: String::new(),
                    event_type: StreamEventType::Done,
                    tool_call: None,
                    finish_reason: None,
                    usage: Some(Usage::default()),
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                })])
            }
        }
        ProviderScenario::StreamEndsWithoutDoneThenComplete => {
            if call_index == 0 {
                Ok(vec![Ok(TextStreamDelta {
                    text: String::new(),
                    event_type: StreamEventType::ToolCallDelta,
                    tool_call: Some(AgentToolCall {
                        id: "fallback-read-1".to_string(),
                        name: "read".to_string(),
                        arguments: serde_json::json!({ "path": "fallback" }),
                        recipient: None,
                    }),
                    finish_reason: None,
                    usage: None,
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                })])
            } else {
                Ok(vec![Ok(TextStreamDelta {
                    text: String::new(),
                    event_type: StreamEventType::Done,
                    tool_call: None,
                    finish_reason: None,
                    usage: Some(Usage::default()),
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                })])
            }
        }
        ProviderScenario::ToolUpdateThenComplete => {
            if call_index == 0 {
                Ok(vec![
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: Some(AgentToolCall {
                            id: "update-tool-1".to_string(),
                            name: "update_tool".to_string(),
                            arguments: serde_json::json!({ "path": "README.md" }),
                            recipient: None,
                        }),
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Done,
                        tool_call: None,
                        finish_reason: None,
                        usage: Some(Usage::default()),
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                ])
            } else {
                Ok(vec![
                    Ok(TextStreamDelta {
                        text: "done".to_string(),
                        event_type: StreamEventType::TextDelta,
                        tool_call: None,
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Done,
                        tool_call: None,
                        finish_reason: None,
                        usage: Some(Usage::default()),
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                ])
            }
        }
        _ => unreachable!(),
    }
}
