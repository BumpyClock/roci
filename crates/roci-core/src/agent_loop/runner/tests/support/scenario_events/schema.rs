use super::super::ProviderScenario;
use crate::error::RociError;
use crate::types::{AgentToolCall, StreamEventType, TextStreamDelta, Usage};

pub(super) fn events_for_scenario(
    scenario: ProviderScenario,
    call_index: usize,
) -> Result<Vec<Result<TextStreamDelta, RociError>>, RociError> {
    let args = match scenario {
        ProviderScenario::SchemaToolBadArgs => serde_json::json!({}),
        ProviderScenario::SchemaToolValidArgs => serde_json::json!({ "path": "/tmp/test" }),
        ProviderScenario::SchemaToolTypeMismatch => serde_json::json!({ "path": 42 }),
        _ => unreachable!(),
    };

    if call_index == 0 {
        Ok(vec![
            Ok(TextStreamDelta {
                text: String::new(),
                event_type: StreamEventType::ToolCallDelta,
                tool_call: Some(AgentToolCall {
                    id: "schema-call-1".to_string(),
                    name: "schema_tool".to_string(),
                    arguments: args,
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
