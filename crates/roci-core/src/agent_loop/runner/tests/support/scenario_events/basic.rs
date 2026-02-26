use super::super::ProviderScenario;
use crate::error::RociError;
use crate::types::{AgentToolCall, StreamEventType, TextStreamDelta, Usage};

pub(super) fn events_for_scenario(
    scenario: ProviderScenario,
    call_index: usize,
) -> Result<Vec<Result<TextStreamDelta, RociError>>, RociError> {
    match scenario {
        ProviderScenario::MissingOptionalFields => Ok(vec![
            Ok(TextStreamDelta {
                text: String::new(),
                event_type: StreamEventType::Reasoning,
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
                tool_call: None,
                finish_reason: None,
                usage: None,
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            }),
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
        ]),
        ProviderScenario::TextThenStreamError => Ok(vec![
            Ok(TextStreamDelta {
                text: "partial".to_string(),
                event_type: StreamEventType::TextDelta,
                tool_call: None,
                finish_reason: None,
                usage: None,
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            }),
            Ok(TextStreamDelta {
                text: "upstream stream failure".to_string(),
                event_type: StreamEventType::Error,
                tool_call: None,
                finish_reason: None,
                usage: None,
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            }),
        ]),
        ProviderScenario::RepeatedToolFailure => Ok(vec![
            Ok(TextStreamDelta {
                text: String::new(),
                event_type: StreamEventType::ToolCallDelta,
                tool_call: Some(AgentToolCall {
                    id: "tool-call-1".to_string(),
                    name: "failing_tool".to_string(),
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
        ]),
        ProviderScenario::RateLimitedThenComplete => {
            if call_index == 0 {
                return Err(RociError::RateLimited {
                    retry_after_ms: Some(1),
                });
            }
            Ok(vec![Ok(TextStreamDelta {
                text: "done".to_string(),
                event_type: StreamEventType::TextDelta,
                tool_call: None,
                finish_reason: None,
                usage: None,
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            })])
        }
        ProviderScenario::RateLimitedExceedsCap => Err(RociError::RateLimited {
            retry_after_ms: Some(50),
        }),
        ProviderScenario::RateLimitedWithoutRetryHint => Err(RociError::RateLimited {
            retry_after_ms: None,
        }),
        _ => unreachable!(),
    }
}
