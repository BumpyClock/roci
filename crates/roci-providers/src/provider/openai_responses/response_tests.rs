use super::super::openai_errors::status_to_openai_error;
use super::response::{
    ResponsesApiResponse, ResponsesChoice, ResponsesChoiceMessage, ResponsesOutputContent,
    ResponsesOutputItem, ResponsesToolCall, ResponsesToolCallFunction,
};
use super::*;
use roci_core::provider::ToolDefinition;

fn settings() -> GenerationSettings {
    GenerationSettings::default()
}

#[test]
fn status_error_maps_context_length_to_typed_code() {
    let body = serde_json::json!({
        "error": {
            "message": "This model's maximum context length is 128000 tokens.",
            "type": "invalid_request_error",
            "code": "context_length_exceeded",
            "param": "input"
        }
    })
    .to_string();

    let error = status_to_openai_error(400, &body);
    match error {
        RociError::Api {
            details: Some(details),
            ..
        } => {
            assert_eq!(
                details.code,
                Some(roci_core::error::ErrorCode::ContextLengthExceeded)
            );
            assert_eq!(
                details.provider_code.as_deref(),
                Some("context_length_exceeded")
            );
            assert_eq!(details.param.as_deref(), Some("input"));
        }
        other => panic!("expected typed API error, got {other:?}"),
    }
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
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
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
fn stream_tool_calls_emit_only_after_finalize_events() {
    let mut state = StreamToolCallState::default();

    state.observe_call("call_1", Some("get_date"));
    state.append_arguments_delta("call_1", r#"{"date":"to"#);
    assert!(state.flush_ready(false).is_empty());

    state.append_arguments_delta("call_1", r#"day"}"#);
    let emitted = state.finalize_call("call_1", None, None);
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].id, "call_1");
    assert_eq!(emitted[0].name, "get_date");
    assert_eq!(emitted[0].arguments, serde_json::json!({"date": "today"}));
}

#[test]
fn stream_tool_calls_preserve_order_until_prior_call_finishes() {
    let mut state = StreamToolCallState::default();

    state.observe_call("call_1", Some("first_tool"));
    state.observe_call("call_2", Some("second_tool"));
    assert!(state
        .finalize_call("call_2", None, Some(r#"{"value":2}"#))
        .is_empty());

    let emitted = state.finalize_call("call_1", None, Some(r#"{"value":1}"#));
    assert_eq!(emitted.len(), 2);
    assert_eq!(emitted[0].id, "call_1");
    assert_eq!(emitted[1].id, "call_2");
}

#[test]
fn stream_tool_calls_avoid_duplicates_and_use_response_output_fallback() {
    let mut state = StreamToolCallState::default();

    state.observe_call("call_1", Some("first_tool"));
    let first_emit = state.finalize_call("call_1", None, Some(r#"{"value":1}"#));
    assert_eq!(first_emit.len(), 1);
    assert!(state
        .finalize_call("call_1", Some("first_tool"), Some(r#"{"value":1}"#))
        .is_empty());

    let response_output = vec![
        serde_json::json!({
            "type": "function_call",
            "call_id": "call_1",
            "name": "first_tool",
            "arguments": r#"{"value":1}"#,
        }),
        serde_json::json!({
            "type": "function_call",
            "call_id": "call_2",
            "name": "second_tool",
            "arguments": r#"{"value":2}"#,
        }),
    ];
    let mut emitted = state.finalize_from_response_output(&response_output);
    emitted.extend(state.flush_ready(true));
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].id, "call_2");
    assert_eq!(emitted[0].name, "second_tool");
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
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let body = provider.build_request_body(&request, false);
    assert_eq!(body["input"][0]["type"], "function_call_output");
    assert_eq!(body["input"][0]["output"], "ok");
}
