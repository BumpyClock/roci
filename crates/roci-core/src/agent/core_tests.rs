use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::{future, stream, StreamExt};

use super::*;
use crate::agent_loop::ApprovalDecision;
use crate::models::ModelCapabilities;
use crate::provider::{ModelProvider, ProviderFactory, ProviderRequest, ProviderResponse};
use crate::tools::{AgentTool, AgentToolParameters, ToolApproval};

struct ToolLoopFactory {
    provider_calls: Arc<AtomicUsize>,
}

impl ProviderFactory for ToolLoopFactory {
    fn provider_keys(&self) -> &[&str] {
        &["stub"]
    }

    fn create(
        &self,
        _config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        Ok(Box::new(ToolLoopProvider {
            model_id: model_id.to_string(),
            provider_calls: self.provider_calls.clone(),
            capabilities: ModelCapabilities::default(),
        }))
    }
}

struct ToolLoopProvider {
    model_id: String,
    provider_calls: Arc<AtomicUsize>,
    capabilities: ModelCapabilities,
}

#[async_trait]
impl ModelProvider for ToolLoopProvider {
    fn provider_name(&self) -> &str {
        "stub"
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        panic!("agent tool path should use runner streaming")
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        let call_index = self.provider_calls.fetch_add(1, Ordering::SeqCst);
        match call_index {
            0 => {
                let tool_count = request.tools.as_ref().map_or(0, Vec::len);
                if tool_count != 1 {
                    return Err(RociError::InvalidState(
                        "tool definitions were not forwarded to provider".to_string(),
                    ));
                }
                Ok(Box::pin(stream::iter(vec![
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: Some(AgentToolCall {
                            id: "call-1".to_string(),
                            name: "lookup".to_string(),
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
                        finish_reason: Some(FinishReason::ToolCalls),
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                ])))
            }
            1 => {
                let saw_tool_result = request.messages.iter().any(|message| {
                    message.content.iter().any(|part| {
                        matches!(
                            part,
                            ContentPart::ToolResult(result)
                                if result.tool_call_id == "call-1"
                                    && result.result == serde_json::json!({ "value": 42 })
                        )
                    })
                });
                if !saw_tool_result {
                    return Err(RociError::InvalidState(
                        "tool result was not fed back to provider".to_string(),
                    ));
                }
                Ok(Box::pin(stream::iter(vec![
                    Ok(TextStreamDelta {
                        text: "lookup complete".to_string(),
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
                        finish_reason: Some(FinishReason::Stop),
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    }),
                ])))
            }
            _ => Err(RociError::InvalidState(
                "provider called too many times".to_string(),
            )),
        }
    }
}

#[tokio::test]
async fn execute_with_tool_uses_runner_tool_path() {
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(ToolLoopFactory {
        provider_calls: provider_calls.clone(),
    }));

    let tool_call_counter = tool_calls.clone();
    let tool = AgentTool::new(
        "lookup",
        "lookup",
        AgentToolParameters::empty(),
        move |_args, _ctx| {
            let tool_call_counter = tool_call_counter.clone();
            async move {
                tool_call_counter.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({ "value": 42 }))
            }
        },
    )
    .with_approval(ToolApproval::safe_read_only());
    let model: LanguageModel = "stub:test-model".parse().expect("test model parses");
    let mut agent = Agent::new(model, Arc::new(registry)).with_tool(Box::new(tool));

    let response = agent.execute("use the lookup tool").await.unwrap();

    assert_eq!(response, "lookup complete");
    assert_eq!(tool_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn stream_with_tool_emits_deltas_and_updates_conversation() {
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(ToolLoopFactory {
        provider_calls: provider_calls.clone(),
    }));

    let tool_call_counter = tool_calls.clone();
    let tool = AgentTool::new(
        "lookup",
        "lookup",
        AgentToolParameters::empty(),
        move |_args, _ctx| {
            let tool_call_counter = tool_call_counter.clone();
            async move {
                tool_call_counter.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({ "value": 42 }))
            }
        },
    )
    .with_approval(ToolApproval::safe_read_only());
    let model: LanguageModel = "stub:test-model".parse().expect("test model parses");
    let mut agent = Agent::new(model, Arc::new(registry)).with_tool(Box::new(tool));

    let mut stream = agent.stream("use the lookup tool").await.unwrap();
    let mut text = String::new();
    while let Some(delta) = stream.next().await {
        text.push_str(&delta.unwrap().text);
    }
    drop(stream);

    assert_eq!(text, "lookup complete");
    assert_eq!(tool_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        agent
            .conversation()
            .messages()
            .last()
            .map(ModelMessage::text),
        Some("lookup complete".to_string())
    );
}

#[tokio::test]
async fn dropping_tool_stream_after_delta_stops_without_extra_provider_calls() {
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(ToolLoopFactory {
        provider_calls: provider_calls.clone(),
    }));

    let tool_call_counter = tool_calls.clone();
    let tool = AgentTool::new(
        "lookup",
        "lookup",
        AgentToolParameters::empty(),
        move |_args, _ctx| {
            let tool_call_counter = tool_call_counter.clone();
            async move {
                tool_call_counter.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({ "value": 42 }))
            }
        },
    )
    .with_approval(ToolApproval::safe_read_only());
    let model: LanguageModel = "stub:test-model".parse().expect("test model parses");
    let mut agent = Agent::new(model, Arc::new(registry)).with_tool(Box::new(tool));

    let mut stream = agent.stream("use the lookup tool").await.unwrap();
    let mut text = String::new();
    while let Some(delta) = stream.next().await {
        let delta = delta.unwrap();
        text.push_str(&delta.text);
        if text == "lookup complete" {
            break;
        }
    }
    drop(stream);

    assert_eq!(text, "lookup complete");
    assert_eq!(tool_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn execute_with_custom_tool_requires_approval_by_default() {
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(ToolLoopFactory {
        provider_calls: provider_calls.clone(),
    }));
    let tool_call_counter = tool_calls.clone();
    let tool = AgentTool::new(
        "lookup",
        "lookup",
        AgentToolParameters::empty(),
        move |_args, _ctx| {
            let tool_call_counter = tool_call_counter.clone();
            async move {
                tool_call_counter.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({ "value": 42 }))
            }
        },
    );
    let model: LanguageModel = "stub:test-model".parse().expect("test model parses");
    let mut agent = Agent::new(model, Arc::new(registry)).with_tool(Box::new(tool));

    let err = agent
        .execute("use the lookup tool")
        .await
        .expect_err("custom tool should require approval");

    assert!(!err.to_string().is_empty());
    assert_eq!(tool_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn execute_with_approval_handler_can_accept_custom_tool() {
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(ToolLoopFactory {
        provider_calls: provider_calls.clone(),
    }));

    let tool_call_counter = tool_calls.clone();
    let tool = AgentTool::new(
        "lookup",
        "lookup",
        AgentToolParameters::empty(),
        move |_args, _ctx| {
            let tool_call_counter = tool_call_counter.clone();
            async move {
                tool_call_counter.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({ "value": 42 }))
            }
        },
    );
    let handler: ApprovalHandler =
        Arc::new(|_request| Box::pin(future::ready(ApprovalDecision::Accept)));
    let model: LanguageModel = "stub:test-model".parse().expect("test model parses");
    let mut agent = Agent::new(model, Arc::new(registry))
        .with_tool(Box::new(tool))
        .with_approval_handler(handler);

    let response = agent.execute("use the lookup tool").await.unwrap();

    assert_eq!(response, "lookup complete");
    assert_eq!(tool_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider_calls.load(Ordering::SeqCst), 2);
}
