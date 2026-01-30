//! Tests for core types.

use pretty_assertions::assert_eq;
use roci::types::*;

#[test]
fn model_message_system() {
    let msg = ModelMessage::system("You are helpful.");
    assert_eq!(msg.role, Role::System);
    assert_eq!(msg.text(), "You are helpful.");
}

#[test]
fn model_message_user() {
    let msg = ModelMessage::user("Hello");
    assert_eq!(msg.role, Role::User);
    assert_eq!(msg.text(), "Hello");
}

#[test]
fn model_message_assistant() {
    let msg = ModelMessage::assistant("Hi there!");
    assert_eq!(msg.role, Role::Assistant);
    assert_eq!(msg.text(), "Hi there!");
}

#[test]
fn model_message_tool_result() {
    let msg = ModelMessage::tool_result("call_1", serde_json::json!({"result": 42}), false);
    assert_eq!(msg.role, Role::Tool);
}

#[test]
fn model_message_serde_roundtrip() {
    let msg = ModelMessage::user("test");
    let json = serde_json::to_string(&msg).unwrap();
    let deserialized: ModelMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.role, Role::User);
    assert_eq!(deserialized.text(), "test");
}

#[test]
fn usage_merge() {
    let mut u1 = Usage {
        input_tokens: 10,
        output_tokens: 20,
        total_tokens: 30,
        ..Default::default()
    };
    let u2 = Usage {
        input_tokens: 5,
        output_tokens: 15,
        total_tokens: 20,
        cache_read_tokens: Some(3),
        ..Default::default()
    };
    u1.merge(&u2);
    assert_eq!(u1.input_tokens, 15);
    assert_eq!(u1.output_tokens, 35);
    assert_eq!(u1.total_tokens, 50);
    assert_eq!(u1.cache_read_tokens, Some(3));
}

#[test]
fn cost_from_usage() {
    let usage = Usage {
        input_tokens: 1_000_000,
        output_tokens: 500_000,
        total_tokens: 1_500_000,
        ..Default::default()
    };
    let cost = Cost::from_usage(&usage, 2.50, 10.0);
    assert!((cost.input_cost - 2.50).abs() < 0.01);
    assert!((cost.output_cost - 5.0).abs() < 0.01);
    assert!((cost.total_cost - 7.50).abs() < 0.01);
}

#[test]
fn generation_settings_builder() {
    let settings = GenerationSettings::builder()
        .max_tokens(1000)
        .temperature(0.7)
        .build();
    assert_eq!(settings.max_tokens, Some(1000));
    assert_eq!(settings.temperature, Some(0.7));
    assert!(settings.top_p.is_none());
}

#[test]
fn finish_reason_display() {
    assert_eq!(FinishReason::Stop.to_string(), "stop");
    assert_eq!(FinishReason::ToolCalls.to_string(), "tool_calls");
}

#[test]
fn reasoning_effort_fromstr() {
    use std::str::FromStr;
    assert_eq!(ReasoningEffort::from_str("high").unwrap(), ReasoningEffort::High);
    assert_eq!(ReasoningEffort::from_str("low").unwrap(), ReasoningEffort::Low);
}
