//! Tests for the generation system using mock provider.

mod common;

use common::MockProvider;
use roci::generation;
use roci::stop::StringStop;
use roci::tools::tool::AgentTool;
use roci::tools::AgentToolParameters;
use roci::types::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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

    let tool: std::sync::Arc<dyn roci::tools::Tool> = std::sync::Arc::new(AgentTool::new(
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
    let provider = std::sync::Arc::new(MockProvider::new("test-model"));
    provider.queue_response("Streamed text here");

    let stream = generation::stream_text(
        provider,
        vec![ModelMessage::user("Stream this")],
        GenerationSettings::default(),
        Vec::new(),
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

#[tokio::test]
async fn stream_text_executes_tool_calls_and_continues() {
    struct StreamToolProvider {
        state: std::sync::Mutex<u8>,
        caps: roci::models::capabilities::ModelCapabilities,
    }

    impl StreamToolProvider {
        fn new() -> Self {
            Self {
                state: std::sync::Mutex::new(0),
                caps: roci::models::capabilities::ModelCapabilities::full(128_000),
            }
        }
    }

    #[async_trait::async_trait]
    impl roci::provider::ModelProvider for StreamToolProvider {
        fn model_id(&self) -> &str {
            "stream-tool"
        }

        fn capabilities(&self) -> &roci::models::capabilities::ModelCapabilities {
            &self.caps
        }

        async fn generate_text(
            &self,
            _request: &roci::provider::ProviderRequest,
        ) -> Result<roci::provider::ProviderResponse, roci::error::RociError> {
            Err(roci::error::RociError::UnsupportedOperation(
                "Not used".into(),
            ))
        }

        async fn stream_text(
            &self,
            _request: &roci::provider::ProviderRequest,
        ) -> Result<
            futures::stream::BoxStream<'static, Result<TextStreamDelta, roci::error::RociError>>,
            roci::error::RociError,
        > {
            let mut state = self.state.lock().unwrap();
            let step = *state;
            *state += 1;
            drop(state);
            if step == 0 {
                let stream = async_stream::stream! {
                    yield Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: Some(AgentToolCall {
                            id: "call_1".to_string(),
                            name: "get_weather".to_string(),
                            arguments: serde_json::json!({"city": "Paris"}),
                            recipient: None,
                        }),
                        finish_reason: None,
                        usage: None,
                    });
                    yield Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Done,
                        tool_call: None,
                        finish_reason: Some(FinishReason::ToolCalls),
                        usage: None,
                    });
                };
                Ok(Box::pin(stream))
            } else {
                let stream = async_stream::stream! {
                    yield Ok(TextStreamDelta {
                        text: "done".to_string(),
                        event_type: StreamEventType::TextDelta,
                        tool_call: None,
                        finish_reason: None,
                        usage: None,
                    });
                    yield Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Done,
                        tool_call: None,
                        finish_reason: Some(FinishReason::Stop),
                        usage: None,
                    });
                };
                Ok(Box::pin(stream))
            }
        }
    }

    let provider = std::sync::Arc::new(StreamToolProvider::new());
    let called = Arc::new(AtomicUsize::new(0));
    let called_ref = Arc::clone(&called);
    let tool: Arc<dyn roci::tools::Tool> = Arc::new(AgentTool::new(
        "get_weather",
        "Get weather for a city",
        AgentToolParameters::object()
            .string("city", "City name", true)
            .build(),
        move |args, _ctx| {
            let called_ref = Arc::clone(&called_ref);
            async move {
                let _ = args.get_str("city")?;
                called_ref.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({"temp": 18}))
            }
        },
    ));

    let stream = generation::stream_text_with_tools(
        provider,
        vec![ModelMessage::user("Use the tool")],
        GenerationSettings::default(),
        &[tool],
        Vec::new(),
    )
    .await
    .unwrap();

    let result = generation::stream::collect_stream(stream).await.unwrap();
    assert_eq!(result.text, "done");
    assert_eq!(called.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stream_text_stops_when_condition_matches() {
    struct StopStreamProvider {
        caps: roci::models::capabilities::ModelCapabilities,
    }

    #[async_trait::async_trait]
    impl roci::provider::ModelProvider for StopStreamProvider {
        fn model_id(&self) -> &str {
            "stop-stream"
        }

        fn capabilities(&self) -> &roci::models::capabilities::ModelCapabilities {
            &self.caps
        }

        async fn generate_text(
            &self,
            _request: &roci::provider::ProviderRequest,
        ) -> Result<roci::provider::ProviderResponse, roci::error::RociError> {
            Err(roci::error::RociError::UnsupportedOperation(
                "Not used".into(),
            ))
        }

        async fn stream_text(
            &self,
            _request: &roci::provider::ProviderRequest,
        ) -> Result<
            futures::stream::BoxStream<'static, Result<TextStreamDelta, roci::error::RociError>>,
            roci::error::RociError,
        > {
            let stream = async_stream::stream! {
                yield Ok(TextStreamDelta {
                    text: "stop-here".to_string(),
                    event_type: StreamEventType::TextDelta,
                    tool_call: None,
                    finish_reason: None,
                    usage: None,
                });
                yield Ok(TextStreamDelta {
                    text: "should-not-see".to_string(),
                    event_type: StreamEventType::TextDelta,
                    tool_call: None,
                    finish_reason: None,
                    usage: None,
                });
            };
            Ok(Box::pin(stream))
        }
    }

    let provider = std::sync::Arc::new(StopStreamProvider {
        caps: roci::models::capabilities::ModelCapabilities::default(),
    });
    let stream = generation::stream_text(
        provider,
        vec![ModelMessage::user("Stop at marker")],
        GenerationSettings::default(),
        vec![Box::new(StringStop::new("stop-here"))],
    )
    .await
    .unwrap();
    let result = generation::stream::collect_stream(stream).await.unwrap();
    assert!(result.text.contains("stop-here"));
    assert!(!result.text.contains("should-not-see"));
}
