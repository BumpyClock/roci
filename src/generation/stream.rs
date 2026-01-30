//! Streaming text generation with stop conditions.

use futures::stream::BoxStream;
use futures::StreamExt;

use crate::error::RociError;
use crate::provider::{ModelProvider, ProviderRequest, ToolDefinition};
use crate::stop::StopCondition;
use crate::tools::arguments::ToolArguments;
use crate::tools::tool::{Tool, ToolExecutionContext};
use crate::types::*;

/// Stream text from a model, applying optional stop conditions.
///
/// Returns a stream of text deltas. Stop conditions can halt the stream early.
pub async fn stream_text(
    provider: std::sync::Arc<dyn ModelProvider>,
    messages: Vec<ModelMessage>,
    settings: GenerationSettings,
    stop_conditions: Vec<Box<dyn StopCondition>>,
) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
    stream_text_with_tools(provider, messages, settings, &[], stop_conditions).await
}

/// Stream text from a model with tool calling and stop conditions.
pub async fn stream_text_with_tools(
    provider: std::sync::Arc<dyn ModelProvider>,
    messages: Vec<ModelMessage>,
    settings: GenerationSettings,
    tools: &[std::sync::Arc<dyn Tool>],
    stop_conditions: Vec<Box<dyn StopCondition>>,
) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
    const MAX_TOOL_ITERATIONS: usize = 20;
    let tool_defs: Option<Vec<ToolDefinition>> = if tools.is_empty() {
        None
    } else {
        Some(
            tools
                .iter()
                .map(|t| ToolDefinition {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters: t.parameters().schema.clone(),
                })
                .collect(),
        )
    };
    let tools = tools.to_vec();
    let stop_conditions = stop_conditions;
    let mut messages = messages;
    let stream = async_stream::stream! {
        let mut accumulated_text = String::new();
        for cond in &stop_conditions {
            cond.reset().await;
        }
        let mut iteration = 0;
        loop {
            iteration += 1;
            if iteration > MAX_TOOL_ITERATIONS {
                yield Ok(TextStreamDelta {
                    text: String::new(),
                    event_type: StreamEventType::Done,
                    tool_call: None,
                    finish_reason: Some(FinishReason::Length),
                    usage: None,
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                });
                break;
            }
            let request = ProviderRequest {
                messages: messages.clone(),
                settings: settings.clone(),
                tools: tool_defs.clone(),
                response_format: settings.response_format.clone(),
            };
            let mut inner = match provider.stream_text(&request).await {
                Ok(stream) => stream,
                Err(e) => {
                    yield Err(e);
                    break;
                }
            };
            let mut iteration_text = String::new();
            let mut tool_calls: Vec<AgentToolCall> = Vec::new();
            let mut pending_done: Option<TextStreamDelta> = None;
            let mut stop_triggered = false;
            while let Some(item) = inner.next().await {
                match item {
                    Ok(delta) => {
                        let event_type = delta.event_type;
                        let delta_text = delta.text.clone();
                        match event_type {
                            StreamEventType::ToolCallDelta => {
                                if let Some(tc) = delta.tool_call.clone() {
                                    tool_calls.push(tc);
                                }
                                yield Ok(delta);
                            }
                            StreamEventType::Done => {
                                pending_done = Some(delta);
                            }
                            _ => {
                                if !delta_text.is_empty() {
                                    iteration_text.push_str(&delta_text);
                                    accumulated_text.push_str(&delta_text);
                                }
                                yield Ok(delta);
                            }
                        }
                        if matches!(event_type, StreamEventType::TextDelta) {
                            for cond in &stop_conditions {
                                if cond
                                    .should_stop(&accumulated_text, Some(&delta_text))
                                    .await
                                {
                                    stop_triggered = true;
                                    break;
                                }
                            }
                            if stop_triggered {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        yield Err(e);
                        return;
                    }
                }
            }
            if stop_triggered {
                yield Ok(TextStreamDelta {
                    text: String::new(),
                    event_type: StreamEventType::Done,
                    tool_call: None,
                    finish_reason: Some(FinishReason::Stop),
                    usage: None,
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                });
                break;
            }
            if !tool_calls.is_empty() {
                let mut assistant_content: Vec<ContentPart> = Vec::new();
                if !iteration_text.is_empty() {
                    assistant_content.push(ContentPart::Text { text: iteration_text });
                }
                for tc in &tool_calls {
                    assistant_content.push(ContentPart::ToolCall(tc.clone()));
                }
                messages.push(ModelMessage {
                    role: Role::Assistant,
                    content: assistant_content,
                    name: None,
                    timestamp: Some(chrono::Utc::now()),
                });
                let ctx = ToolExecutionContext::default();
                for tc in &tool_calls {
                    let tool = tools.iter().find(|t| t.name() == tc.name);
                    let result = match tool {
                        Some(t) => {
                            let args = ToolArguments::new(tc.arguments.clone());
                            match t.execute(&args, &ctx).await {
                                Ok(val) => message::AgentToolResult {
                                    tool_call_id: tc.id.clone(),
                                    result: val,
                                    is_error: false,
                                },
                                Err(e) => message::AgentToolResult {
                                    tool_call_id: tc.id.clone(),
                                    result: serde_json::json!({"error": e.to_string()}),
                                    is_error: true,
                                },
                            }
                        }
                        None => message::AgentToolResult {
                            tool_call_id: tc.id.clone(),
                            result: serde_json::json!({"error": format!("Tool '{}' not found", tc.name)}),
                            is_error: true,
                        },
                    };
                    messages.push(ModelMessage::tool_result(
                        result.tool_call_id.clone(),
                        result.result,
                        result.is_error,
                    ));
                }
                continue;
            }
            if let Some(done) = pending_done {
                yield Ok(done);
            }
            break;
        }
    };
    Ok(Box::pin(stream))
}

/// Collect a stream into a final result.
pub async fn collect_stream(
    mut stream: BoxStream<'static, Result<TextStreamDelta, RociError>>,
) -> Result<StreamTextResult, RociError> {
    let mut text = String::new();
    let mut usage = Usage::default();
    let mut finish_reason = None;

    while let Some(delta) = stream.next().await {
        let delta = delta?;
        text.push_str(&delta.text);
        if let Some(u) = delta.usage {
            usage = u;
        }
        if let Some(fr) = delta.finish_reason {
            finish_reason = Some(fr);
        }
    }

    Ok(StreamTextResult {
        text,
        usage,
        finish_reason,
    })
}
