//! Tests for the generation system using mock provider.

mod common;

use common::MockProvider;
use roci::generation;
use roci::tools::tool::AgentTool;
use roci::tools::AgentToolParameters;
use roci::types::*;

#[tokio::test]
async fn generate_text_simple() {
    let provider = MockProvider::new("test-model");
    provider.queue_response("Hello from mock!");

    let result = generation::generate_text(
        &provider,
        vec![ModelMessage::user("Hi")],
        GenerationSettings::default(),
        &[],
    )
    .await
    .unwrap();

    assert_eq!(result.text, "Hello from mock!");
    assert_eq!(result.steps.len(), 1);
    assert_eq!(result.finish_reason, Some(FinishReason::Stop));
}

#[tokio::test]
async fn generate_text_with_tool_loop() {
    let provider = MockProvider::new("test-model");

    // First response: tool call
    provider.queue_tool_call(
        "call_1",
        "get_weather",
        serde_json::json!({"city": "London"}),
    );
    // Second response: final text (after tool result)
    provider.queue_response("The weather in London is sunny.");

    let tool: Box<dyn roci::tools::Tool> = Box::new(AgentTool::new(
        "get_weather",
        "Get weather for a city",
        AgentToolParameters::object()
            .string("city", "City name", true)
            .build(),
        |args, _ctx| async move {
            let city = args.get_str("city")?;
            Ok(serde_json::json!({"temp": 22, "condition": "sunny", "city": city}))
        },
    ));

    let result = generation::generate_text(
        &provider,
        vec![ModelMessage::user("What's the weather in London?")],
        GenerationSettings::default(),
        &[tool],
    )
    .await
    .unwrap();

    assert_eq!(result.text, "The weather in London is sunny.");
    assert_eq!(result.steps.len(), 2); // tool call step + final step
    assert_eq!(result.steps[0].tool_calls.len(), 1);
    assert_eq!(result.steps[0].tool_results.len(), 1);
    assert!(!result.steps[0].tool_results[0].is_error);
}

#[tokio::test]
async fn generate_text_tool_not_found() {
    let provider = MockProvider::new("test-model");

    // Tool call for a tool that doesn't exist
    provider.queue_tool_call("call_1", "nonexistent", serde_json::json!({}));
    provider.queue_response("I couldn't find that tool.");

    let result = generation::generate_text(
        &provider,
        vec![ModelMessage::user("Use nonexistent tool")],
        GenerationSettings::default(),
        &[], // no tools provided
    )
    .await
    .unwrap();

    assert_eq!(result.steps.len(), 2);
    assert!(result.steps[0].tool_results[0].is_error);
}

#[tokio::test]
async fn stream_text_collects() {
    let provider = MockProvider::new("test-model");
    provider.queue_response("Streamed text here");

    let stream = generation::stream_text(
        &provider,
        vec![ModelMessage::user("Stream this")],
        GenerationSettings::default(),
        &[],
    )
    .await
    .unwrap();

    let result = generation::stream::collect_stream(stream).await.unwrap();
    assert!(result.text.contains("Streamed text here"));
    assert_eq!(result.finish_reason, Some(FinishReason::Stop));
}

#[tokio::test]
async fn generate_object_parses_json() {
    let provider = MockProvider::new("test-model");
    provider.queue_response(r#"{"name": "Alice", "age": 30}"#);

    #[derive(serde::Deserialize, Debug)]
    struct Person {
        name: String,
        age: u32,
    }

    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "age": {"type": "integer"}
        },
        "required": ["name", "age"]
    });

    let result = generation::generate_object::<Person>(
        &provider,
        vec![ModelMessage::user("Generate a person")],
        GenerationSettings::default(),
        schema,
        "Person",
    )
    .await
    .unwrap();

    assert_eq!(result.object.name, "Alice");
    assert_eq!(result.object.age, 30);
}
