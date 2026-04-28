use super::support::*;
use super::*;
use crate::agent_loop::AgentEvent;
use crate::models::ModelCapabilities;
use crate::provider::{ModelProvider, ProviderFactory, ProviderRequest, ProviderResponse};
use crate::tools::tool::Tool;
use crate::tools::{
    AgentTool, AgentToolParameters, Question, ToolApproval, UserInputRequest, UserInputResponse,
};
use crate::types::{AgentToolCall, StreamEventType, TextStreamDelta, Usage};
use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};

struct AskUserFactory {
    calls: Arc<AtomicUsize>,
}

impl ProviderFactory for AskUserFactory {
    fn provider_keys(&self) -> &[&str] {
        &["stub"]
    }

    fn create(
        &self,
        _config: &RociConfig,
        _provider_key: &str,
        _model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        Ok(Box::new(AskUserProvider {
            calls: self.calls.clone(),
            capabilities: ModelCapabilities::default(),
        }))
    }
}

struct AskUserProvider {
    calls: Arc<AtomicUsize>,
    capabilities: ModelCapabilities,
}

#[async_trait]
impl ModelProvider for AskUserProvider {
    fn provider_name(&self) -> &str {
        "stub"
    }

    fn model_id(&self) -> &str {
        "ask-user-runtime"
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        Err(RociError::UnsupportedOperation(
            "stream-only ask-user test provider".to_string(),
        ))
    }

    async fn stream_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
        let events = if call_index == 0 {
            vec![
                Ok(TextStreamDelta {
                    text: String::new(),
                    event_type: StreamEventType::ToolCallDelta,
                    tool_call: Some(AgentToolCall {
                        id: "ask-user-call-1".to_string(),
                        name: "ask_user".to_string(),
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
            ]
        } else {
            vec![
                Ok(TextStreamDelta {
                    text: "unit confirmed".to_string(),
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
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
}

#[tokio::test]
async fn prompt_emits_user_input_event_and_submit_user_input_unblocks_tool() {
    let event_requests = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(AskUserFactory {
        calls: Arc::new(AtomicUsize::new(0)),
    }));
    let registry = Arc::new(registry);

    let ask_user_tool: Arc<dyn Tool> = Arc::new(
        AgentTool::new(
            "ask_user",
            "ask user test tool",
            AgentToolParameters::empty(),
            |_args, ctx| async move {
                let callback = ctx.request_user_input.clone().ok_or_else(|| {
                    RociError::InvalidState("missing request_user_input".to_string())
                })?;
                let response = callback(UserInputRequest {
                    request_id: uuid::Uuid::new_v4(),
                    tool_call_id: "ask-user-call-1".to_string(),
                    questions: vec![Question {
                        id: "temp_unit".to_string(),
                        text: "C or F?".to_string(),
                        options: None,
                    }],
                    timeout_ms: Some(1_000),
                })
                .await
                .map_err(|err| RociError::InvalidState(err.to_string()))?;
                Ok(serde_json::json!({
                    "answer": response.answers.first().map(|answer| answer.content.clone())
                }))
            },
        )
        .with_approval(ToolApproval::safe_host_input()),
    );

    let agent_slot: Arc<Mutex<Option<Arc<AgentRuntime>>>> = Arc::new(Mutex::new(None));
    let mut config = test_agent_config();
    config.model = "stub:ask-user-runtime".parse().expect("stub model parses");
    config.tools = vec![ask_user_tool];
    config.event_sink = Some({
        let event_requests = event_requests.clone();
        let agent_slot = agent_slot.clone();
        Arc::new(move |event| {
            if let AgentEvent::UserInputRequested { request } = event {
                event_requests
                    .lock()
                    .expect("event lock")
                    .push(request.clone());
                if let Some(agent) = agent_slot.lock().expect("agent lock").clone() {
                    tokio::spawn(async move {
                        let _ = agent
                            .submit_user_input(UserInputResponse {
                                request_id: request.request_id,
                                answers: vec![crate::tools::Answer {
                                    question_id: "temp_unit".to_string(),
                                    content: "C".to_string(),
                                }],
                                canceled: false,
                            })
                            .await;
                    });
                }
            }
        })
    });

    let agent = Arc::new(AgentRuntime::new(registry, test_config(), config));
    *agent_slot.lock().expect("agent lock") = Some(agent.clone());

    let result = agent
        .prompt("ask me a unit")
        .await
        .expect("prompt should succeed");

    assert_eq!(result.status, RunStatus::Completed);
    assert!(
        result
            .messages
            .iter()
            .any(|message| message.text().contains("unit confirmed")),
        "expected follow-up provider response after user input"
    );

    let requests = event_requests.lock().expect("event lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].questions[0].id, "temp_unit");
}

#[tokio::test]
async fn abort_while_waiting_for_user_input_unblocks_run() {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(AskUserFactory {
        calls: Arc::new(AtomicUsize::new(0)),
    }));
    let registry = Arc::new(registry);

    let ask_user_tool: Arc<dyn Tool> = Arc::new(
        AgentTool::new(
            "ask_user",
            "ask user test tool",
            AgentToolParameters::empty(),
            |_args, ctx| async move {
                let callback = ctx.request_user_input.clone().ok_or_else(|| {
                    RociError::InvalidState("missing request_user_input".to_string())
                })?;
                let response = callback(UserInputRequest {
                    request_id: uuid::Uuid::new_v4(),
                    tool_call_id: "ask-user-call-1".to_string(),
                    questions: vec![Question {
                        id: "abort_unit".to_string(),
                        text: "Abort me?".to_string(),
                        options: None,
                    }],
                    timeout_ms: None,
                })
                .await
                .map_err(|err| RociError::InvalidState(err.to_string()))?;
                Ok(serde_json::json!({
                    "answer": response.answers.first().map(|answer| answer.content.clone())
                }))
            },
        )
        .with_approval(ToolApproval::safe_host_input()),
    );

    let (request_seen_tx, request_seen_rx) = oneshot::channel();
    let request_seen_tx = Arc::new(Mutex::new(Some(request_seen_tx)));

    let mut config = test_agent_config();
    config.model = "stub:ask-user-runtime".parse().expect("stub model parses");
    config.tools = vec![ask_user_tool];
    config.event_sink = Some({
        let request_seen_tx = Arc::clone(&request_seen_tx);
        Arc::new(move |event| {
            if let AgentEvent::UserInputRequested { request } = event {
                if let Some(tx) = request_seen_tx.lock().expect("event lock").take() {
                    let _ = tx.send(request);
                }
            }
        })
    });

    let agent = Arc::new(AgentRuntime::new(registry, test_config(), config));
    let prompt_agent = Arc::clone(&agent);
    let prompt_task = tokio::spawn(async move { prompt_agent.prompt("ask me a unit").await });

    let request = timeout(std::time::Duration::from_secs(1), request_seen_rx)
        .await
        .expect("user input request should arrive")
        .expect("request sender should not drop");
    assert_eq!(request.questions[0].id, "abort_unit");

    sleep(std::time::Duration::from_millis(10)).await;
    assert!(agent.abort().await, "abort should signal running prompt");

    let result = timeout(std::time::Duration::from_secs(1), prompt_task)
        .await
        .expect("prompt task should finish after abort")
        .expect("prompt task join should succeed")
        .expect("prompt should resolve to run result");

    assert_eq!(result.status, RunStatus::Canceled);
}
