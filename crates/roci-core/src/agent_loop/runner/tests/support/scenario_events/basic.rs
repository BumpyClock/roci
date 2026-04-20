use super::super::ProviderScenario;
use crate::error::RociError;
use crate::error::{ErrorCode, ErrorDetails};
use crate::types::{AgentToolCall, StreamEventType, TextStreamDelta, Usage};

fn typed_overflow_error() -> RociError {
    RociError::api_with_details(
        400,
        "context length exceeded",
        ErrorDetails {
            code: Some(ErrorCode::ContextLengthExceeded),
            provider_code: Some("context_length_exceeded".to_string()),
            param: None,
            request_id: None,
        },
    )
}

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
        ProviderScenario::ImmediateStreamError => Ok(vec![Err(RociError::Stream(
            "simulated immediate stream failure".to_string(),
        ))]),
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
        ProviderScenario::RetryableTimeoutThenComplete => {
            if call_index == 0 {
                return Err(RociError::Timeout(10));
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
        ProviderScenario::RetryableTimeoutExhausted => Err(RociError::Timeout(10)),
        ProviderScenario::ContextOverflowThenComplete => {
            if call_index == 0 {
                return Err(typed_overflow_error());
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
        ProviderScenario::ContextOverflowAlways => Err(typed_overflow_error()),
        ProviderScenario::UntypedOverflowError => {
            Err(RociError::api(400, "context length exceeded"))
        }
        ProviderScenario::TextOnlyWithUsage => Ok(vec![
            Ok(TextStreamDelta {
                text: "hello".to_string(),
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
                usage: Some(Usage {
                    input_tokens: 50,
                    output_tokens: 10,
                    total_tokens: 60,
                    ..Usage::default()
                }),
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            }),
        ]),
        ProviderScenario::TextWithUsageThenStreamError => Ok(vec![
            Ok(TextStreamDelta {
                text: "partial".to_string(),
                event_type: StreamEventType::TextDelta,
                tool_call: None,
                finish_reason: None,
                usage: Some(Usage {
                    input_tokens: 30,
                    output_tokens: 5,
                    total_tokens: 35,
                    ..Usage::default()
                }),
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            }),
            Err(RociError::Stream(
                "simulated mid-stream failure".to_string(),
            )),
        ]),
        ProviderScenario::ToolCallWithUsageThenTextWithUsage => {
            if call_index == 0 {
                // First call: tool call + usage (input=50, output=10).
                Ok(vec![
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: Some(AgentToolCall {
                            id: "tc-anchor-1".to_string(),
                            name: "noop_tool".to_string(),
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
                        usage: Some(Usage {
                            input_tokens: 50,
                            output_tokens: 10,
                            total_tokens: 60,
                            ..Usage::default()
                        }),
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                ])
            } else {
                // Subsequent calls: text "done" + usage (input=60, output=5).
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
                        usage: Some(Usage {
                            input_tokens: 60,
                            output_tokens: 5,
                            total_tokens: 65,
                            ..Usage::default()
                        }),
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
