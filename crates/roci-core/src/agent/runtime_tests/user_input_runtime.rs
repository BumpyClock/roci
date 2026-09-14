use super::chat::{
    AgentRuntimeError, AgentRuntimeEvent, AgentRuntimeEventPayload, AgentRuntimeEventStore,
    ChatProjector, ChatRuntimeConfig, InMemoryAgentRuntimeEventStore, RuntimeCursor, ThreadId,
    TurnStatus,
};
use super::support::*;
use super::*;
use crate::agent_loop::AgentEvent;
use crate::models::ModelCapabilities;
use crate::provider::{ModelProvider, ProviderFactory, ProviderRequest, ProviderResponse};
use crate::tools::tool::Tool;
use crate::tools::{
    AgentTool, AgentToolParameters, AskUserPrompt, ToolSafetyKind, ToolSafetyPlan,
    ToolSafetySummary, UserInputRequest, UserInputResponse, UserInputResult,
};
use crate::types::{AgentToolCall, StreamEventType, TextStreamDelta, Usage};
use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};

struct AskUserFactory {
    calls: Arc<AtomicUsize>,
}

fn host_input_safety_summary() -> ToolSafetySummary {
    ToolSafetySummary {
        read_only_by_default: false,
        destructive_by_default: false,
        concurrency_safe_by_default: false,
        approval_kind: ToolSafetyKind::Other,
    }
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
                        called_as: None,
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
                    prompt: AskUserPrompt::Question {
                        id: "temp_unit".to_string(),
                        question: "C or F?".to_string(),
                        placeholder: None,
                        default: None,
                        multiline: false,
                    },
                    timeout_ms: Some(1_000),
                })
                .await
                .map_err(|err| RociError::InvalidState(err.to_string()))?;
                let UserInputResult::Question { answer } = response.result else {
                    return Err(RociError::InvalidState(
                        "expected question answer".to_string(),
                    ));
                };
                Ok(serde_json::json!({ "answer": answer }))
            },
        )
        .with_static_safety(ToolSafetyPlan::host_input(), host_input_safety_summary()),
    );

    let agent_slot: Arc<Mutex<Option<Arc<AgentRuntime>>>> = Arc::new(Mutex::new(None));
    let mut config = test_agent_config();
    config.candidates = vec!["stub:ask-user-runtime".parse().expect("stub model parses")];
    config.tools = vec![ask_user_tool];
    config.event_sink = Some({
        let event_requests = event_requests.clone();
        let agent_slot = agent_slot.clone();
        Arc::new(move |event| {
            if let AgentEvent::HumanInteractionRequested { request } = event {
                let request = request
                    .to_user_input()
                    .expect("human interaction should be ask_user");
                event_requests
                    .lock()
                    .expect("event lock")
                    .push(request.clone());
                if let Some(agent) = agent_slot.lock().expect("agent lock").clone() {
                    tokio::spawn(async move {
                        let _ = agent
                            .submit_user_input(UserInputResponse {
                                request_id: request.request_id,
                                result: UserInputResult::Question {
                                    answer: "C".to_string(),
                                },
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
    assert_eq!(requests[0].prompt.id(), "temp_unit");
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
                    prompt: AskUserPrompt::Question {
                        id: "abort_unit".to_string(),
                        question: "Abort me?".to_string(),
                        placeholder: None,
                        default: None,
                        multiline: false,
                    },
                    timeout_ms: None,
                })
                .await
                .map_err(|err| RociError::InvalidState(err.to_string()))?;
                let UserInputResult::Question { answer } = response.result else {
                    return Err(RociError::InvalidState(
                        "expected question answer".to_string(),
                    ));
                };
                Ok(serde_json::json!({ "answer": answer }))
            },
        )
        .with_static_safety(ToolSafetyPlan::host_input(), host_input_safety_summary()),
    );

    let (request_seen_tx, request_seen_rx) = oneshot::channel();
    let request_seen_tx = Arc::new(Mutex::new(Some(request_seen_tx)));

    let mut config = test_agent_config();
    config.candidates = vec!["stub:ask-user-runtime".parse().expect("stub model parses")];
    config.tools = vec![ask_user_tool];
    config.event_sink = Some({
        let request_seen_tx = Arc::clone(&request_seen_tx);
        Arc::new(move |event| {
            if let AgentEvent::HumanInteractionRequested { request } = event {
                if let Some(tx) = request_seen_tx.lock().expect("event lock").take() {
                    let request = request
                        .to_user_input()
                        .expect("human interaction should be ask_user");
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
    assert_eq!(request.prompt.id(), "abort_unit");

    sleep(std::time::Duration::from_millis(10)).await;
    assert!(agent.abort().await, "abort should signal running prompt");

    let result = timeout(std::time::Duration::from_secs(1), prompt_task)
        .await
        .expect("prompt task should finish after abort")
        .expect("prompt task join should succeed")
        .expect("prompt should resolve to run result");

    assert_eq!(result.status, RunStatus::Canceled);
}

struct FailingInteractionStore {
    inner: InMemoryAgentRuntimeEventStore,
    failed: AtomicBool,
}

#[async_trait]
impl AgentRuntimeEventStore for FailingInteractionStore {
    async fn append(&self, event: AgentRuntimeEvent) -> Result<RuntimeCursor, AgentRuntimeError> {
        Ok(self.append_batch(vec![event]).await?.remove(0))
    }

    async fn append_batch(
        &self,
        events: Vec<AgentRuntimeEvent>,
    ) -> Result<Vec<RuntimeCursor>, AgentRuntimeError> {
        if events.iter().any(|event| {
            matches!(
                event.payload,
                AgentRuntimeEventPayload::HumanInteractionRequested { .. }
            )
        }) && !self.failed.swap(true, Ordering::SeqCst)
        {
            return Err(AgentRuntimeError::ProjectionFailed {
                message: "injected human interaction append failure".into(),
            });
        }
        self.inner.append_batch(events).await
    }

    async fn events_after(
        &self,
        cursor: RuntimeCursor,
    ) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
        self.inner.events_after(cursor).await
    }

    async fn invalidate_thread(
        &self,
        thread_id: ThreadId,
        latest_seq: u64,
    ) -> Result<(), AgentRuntimeError> {
        self.inner.invalidate_thread(thread_id, latest_seq).await
    }
}

#[tokio::test]
async fn failed_human_interaction_append_unblocks_run_without_user_response() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(AskUserFactory {
        calls: calls.clone(),
    }));
    let coordinator = Arc::new(HumanInteractionCoordinator::new());
    let store = Arc::new(FailingInteractionStore {
        inner: InMemoryAgentRuntimeEventStore::new(),
        failed: AtomicBool::new(false),
    });
    let ask_user_tool: Arc<dyn Tool> = Arc::new(
        AgentTool::new(
            "ask_user",
            "ask without a timeout",
            AgentToolParameters::empty(),
            |_, ctx| async move {
                let callback = ctx
                    .request_user_input
                    .as_ref()
                    .expect("runtime should provide user input");
                let response = callback(UserInputRequest {
                    request_id: uuid::Uuid::new_v4(),
                    tool_call_id: "ask-user-call-1".into(),
                    prompt: AskUserPrompt::Question {
                        id: "invisible_question".into(),
                        question: "Will never be shown".into(),
                        placeholder: None,
                        default: None,
                        multiline: false,
                    },
                    timeout_ms: None,
                })
                .await
                .map_err(|error| RociError::InvalidState(error.to_string()))?;
                Ok(serde_json::to_value(response).unwrap())
            },
        )
        .with_static_safety(ToolSafetyPlan::host_input(), host_input_safety_summary()),
    );
    let mut config = test_agent_config();
    config.candidates = vec!["stub:ask-user-runtime".parse().unwrap()];
    config.tools = vec![ask_user_tool];
    config.user_input_timeout_ms = None;
    config.human_interaction_coordinator = Some(coordinator.clone());
    config.chat.event_store = Some(store.clone());
    let agent = AgentRuntime::new(Arc::new(registry), test_config(), config);
    let thread_id = agent.default_thread_id();

    let error = timeout(
        std::time::Duration::from_secs(2),
        agent.prompt("ask a question"),
    )
    .await
    .expect("failed request publication must not wait for an invisible user response")
    .expect_err("request publication failure should propagate");

    assert!(error
        .to_string()
        .contains("injected human interaction append failure"));
    assert!(
        store.failed.load(Ordering::SeqCst),
        "must exercise the requested-event append failure"
    );
    assert_eq!(agent.state().await, AgentState::Idle);
    assert!(
        coordinator.pending_requests().await.is_empty(),
        "aborted interaction must not remain in the coordinator"
    );
    let snapshot = agent.read_thread(thread_id).await.unwrap();
    assert_eq!(snapshot.turns.len(), 1);
    assert_eq!(snapshot.turns[0].status, TurnStatus::Failed);
    assert!(snapshot.human_interactions.is_empty());
    let replay = agent
        .subscribe(Some(RuntimeCursor::new(thread_id, 0)))
        .await
        .replay()
        .unwrap();
    assert!(!replay.iter().any(|event| matches!(
        event.payload,
        AgentRuntimeEventPayload::HumanInteractionRequested { .. }
    )));
    let reconstructed = ChatProjector::from_events(
        ChatRuntimeConfig {
            default_thread_id: Some(thread_id),
            ..Default::default()
        },
        replay,
    )
    .unwrap();
    assert_eq!(agent.read_snapshot().await, reconstructed.read_snapshot());

    let next = timeout(
        std::time::Duration::from_secs(2),
        agent.prompt("continue after storage failure"),
    )
    .await
    .expect("next prompt should not be stranded")
    .expect("store recovers after one failure");
    assert_eq!(next.status, RunStatus::Completed);
    assert!(next
        .messages
        .iter()
        .any(|message| message.text() == "unit confirmed"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
