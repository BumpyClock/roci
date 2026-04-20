//! Tests for SubagentSupervisor lifecycle and public runtime behavior.
//!
//! Drop-behavior tests and internal-access tests live in
//! `supervisor.rs`'s own `mod tests` block. These tests exercise the
//! public API only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use uuid::Uuid;

use crate::agent::runtime::AgentConfig;
use crate::agent::subagents::profiles::SubagentProfileRegistry;
use crate::agent::subagents::supervisor::SubagentSupervisor;
use crate::agent::subagents::types::SubagentEvent;
use crate::agent::subagents::types::{
    ModelCandidate, SnapshotMode, SubagentContext, SubagentInput, SubagentProfile, SubagentSpec,
    SubagentStatus, SubagentSupervisorConfig,
};
use crate::agent::subagents::SubagentPromptPolicy;
use crate::agent_loop::AgentEvent;
use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::{LanguageModel, ModelCapabilities};
use crate::provider::{
    ModelProvider, ProviderFactory, ProviderRegistry, ProviderRequest, ProviderResponse,
};
use crate::tools::tool::Tool;
use crate::tools::{AgentTool, AgentToolParameters, Question, UserInputRequest};
use crate::types::{ModelMessage, Role, StreamEventType, TextStreamDelta, Usage};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_test_model() -> LanguageModel {
    LanguageModel::Known {
        provider_key: "test".into(),
        model_id: "test-model".into(),
    }
}

fn make_base_config() -> AgentConfig {
    use crate::agent::runtime::QueueDrainMode;
    use crate::agent_loop::runner::RetryBackoffPolicy;
    use crate::resource::CompactionSettings;
    use crate::types::GenerationSettings;

    AgentConfig {
        model: make_test_model(),
        system_prompt: None,
        tools: Vec::new(),
        dynamic_tool_providers: Vec::new(),
        settings: GenerationSettings::default(),
        transform_context: None,
        convert_to_llm: None,
        before_agent_start: None,
        event_sink: None,
        session_id: None,
        steering_mode: QueueDrainMode::All,
        follow_up_mode: QueueDrainMode::All,
        transport: None,
        max_retry_delay_ms: None,
        retry_backoff: RetryBackoffPolicy::default(),
        api_key_override: None,
        provider_headers: reqwest::header::HeaderMap::new(),
        provider_metadata: HashMap::new(),
        provider_payload_callback: None,
        get_api_key: None,
        compaction: CompactionSettings::default(),
        session_before_compact: None,
        session_before_tree: None,
        pre_tool_use: None,
        post_tool_use: None,
        user_input_timeout_ms: None,
        #[cfg(feature = "agent")]
        user_input_coordinator: None,
        context_budget: None,
    }
}

fn make_supervisor() -> SubagentSupervisor {
    let registry = Arc::new(ProviderRegistry::new());
    let roci_config = RociConfig::default();
    let base_config = make_base_config();
    let sup_config = SubagentSupervisorConfig::default();
    let profile_registry = SubagentProfileRegistry::with_builtins();
    SubagentSupervisor::new(
        registry,
        roci_config,
        base_config,
        sup_config,
        profile_registry,
    )
}

fn make_supervisor_with_config(sup_config: SubagentSupervisorConfig) -> SubagentSupervisor {
    let registry = Arc::new(ProviderRegistry::new());
    let roci_config = RociConfig::default();
    let base_config = make_base_config();
    let profile_registry = SubagentProfileRegistry::with_builtins();
    SubagentSupervisor::new(
        registry,
        roci_config,
        base_config,
        sup_config,
        profile_registry,
    )
}

struct RecordingProviderFactory {
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
    response_text: String,
}

impl RecordingProviderFactory {
    fn new(requests: Arc<Mutex<Vec<ProviderRequest>>>, response_text: impl Into<String>) -> Self {
        Self {
            requests,
            response_text: response_text.into(),
        }
    }
}

impl ProviderFactory for RecordingProviderFactory {
    fn provider_keys(&self) -> &[&str] {
        &["test"]
    }

    fn create(
        &self,
        _config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        Ok(Box::new(RecordingProvider {
            provider_key: "test".into(),
            model_id: model_id.to_string(),
            requests: self.requests.clone(),
            response_text: self.response_text.clone(),
            capabilities: ModelCapabilities {
                supports_streaming: false,
                ..ModelCapabilities::default()
            },
        }))
    }
}

struct RecordingProvider {
    provider_key: String,
    model_id: String,
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
    response_text: String,
    capabilities: ModelCapabilities,
}

#[async_trait]
impl ModelProvider for RecordingProvider {
    fn provider_name(&self) -> &str {
        &self.provider_key
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
        Err(RociError::UnsupportedOperation(
            "recording test provider uses stream_text".into(),
        ))
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        self.requests
            .lock()
            .expect("request capture lock should not be poisoned")
            .push(request.clone());

        Ok(Box::pin(futures::stream::iter(vec![
            Ok(TextStreamDelta {
                text: self.response_text.clone(),
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
        ])))
    }
}

fn make_recording_supervisor(
    response_text: &str,
) -> (SubagentSupervisor, Arc<Mutex<Vec<ProviderRequest>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(RecordingProviderFactory::new(
        requests.clone(),
        response_text,
    )));

    let roci_config = RociConfig::default();
    roci_config.set_api_key("test", "test-key".into());

    let mut profile_registry = SubagentProfileRegistry::with_builtins();
    profile_registry.register(SubagentProfile {
        name: "test:dev".into(),
        system_prompt: Some("You are a test sub-agent.".into()),
        models: vec![ModelCandidate {
            provider: "test".into(),
            model: "test-model".into(),
            reasoning_effort: None,
        }],
        ..Default::default()
    });

    let supervisor = SubagentSupervisor::new(
        Arc::new(registry),
        roci_config,
        make_base_config(),
        SubagentSupervisorConfig::default(),
        profile_registry,
    );
    (supervisor, requests)
}

struct BlockingAskUserFactory;

impl ProviderFactory for BlockingAskUserFactory {
    fn provider_keys(&self) -> &[&str] {
        &["test"]
    }

    fn create(
        &self,
        _config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        Ok(Box::new(BlockingAskUserProvider {
            model_id: model_id.to_string(),
            capabilities: ModelCapabilities {
                supports_streaming: false,
                ..ModelCapabilities::default()
            },
        }))
    }
}

struct BlockingAskUserProvider {
    model_id: String,
    capabilities: ModelCapabilities,
}

#[async_trait]
impl ModelProvider for BlockingAskUserProvider {
    fn provider_name(&self) -> &str {
        "test"
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
        Err(RociError::UnsupportedOperation(
            "blocking ask_user test provider uses stream_text".into(),
        ))
    }

    async fn stream_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        Ok(Box::pin(stream::iter(vec![
            Ok(TextStreamDelta {
                text: String::new(),
                event_type: StreamEventType::ToolCallDelta,
                tool_call: Some(crate::types::AgentToolCall {
                    id: "ask-user-call-1".into(),
                    name: "ask_user".into(),
                    arguments: serde_json::json!({
                        "questions": [
                            {
                                "id": "abort_unit",
                                "text": "Abort me?"
                            }
                        ]
                    }),
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
        ])))
    }
}

fn make_blocking_ask_user_supervisor() -> SubagentSupervisor {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(BlockingAskUserFactory));
    let roci_config = RociConfig::default();
    roci_config.set_api_key("test", "test-key".into());

    let ask_user_tool: Arc<dyn Tool> = Arc::new(AgentTool::new(
        "ask_user",
        "ask user test tool",
        AgentToolParameters::empty(),
        |_args, ctx| async move {
            let callback = ctx
                .request_user_input
                .clone()
                .ok_or_else(|| RociError::InvalidState("missing request_user_input".to_string()))?;
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
    ));

    let mut base_config = make_base_config();
    base_config.model = LanguageModel::Known {
        provider_key: "test".into(),
        model_id: "test-model".into(),
    };
    base_config.tools = vec![ask_user_tool];

    let mut profile_registry = SubagentProfileRegistry::with_builtins();
    profile_registry.register(SubagentProfile {
        name: "test:dev".into(),
        system_prompt: Some("You are a test sub-agent.".into()),
        models: vec![ModelCandidate {
            provider: "test".into(),
            model: "test-model".into(),
            reasoning_effort: None,
        }],
        ..Default::default()
    });

    SubagentSupervisor::new(
        Arc::new(registry),
        roci_config,
        base_config,
        SubagentSupervisorConfig::default(),
        profile_registry,
    )
}

fn captured_request_messages(requests: &Arc<Mutex<Vec<ProviderRequest>>>) -> Vec<ModelMessage> {
    let requests = requests
        .lock()
        .expect("request capture lock should not be poisoned");
    assert_eq!(requests.len(), 1, "expected exactly one provider request");
    requests[0].messages.clone()
}

fn assert_test_system_prompt(message: &ModelMessage) {
    let preamble = SubagentPromptPolicy::default_child_preamble();
    assert_eq!(message.role, Role::System);
    assert!(
        message.text().starts_with(preamble),
        "child system prompt should start with the default preamble"
    );
    assert!(
        message.text().contains("You are a test sub-agent."),
        "child system prompt should include the profile prompt"
    );
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

#[test]
fn supervisor_construction_with_default_config() {
    let supervisor = make_supervisor();
    let _rx = supervisor.subscribe();
}

#[test]
fn supervisor_construction_with_custom_config() {
    let config = SubagentSupervisorConfig {
        max_concurrent: 2,
        max_active_children: Some(10),
        default_input_timeout_ms: Some(60_000),
        abort_on_drop: false,
    };
    let _supervisor = make_supervisor_with_config(config);
}

// ---------------------------------------------------------------------------
// list_active
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_active_empty_on_fresh_supervisor() {
    let supervisor = make_supervisor();
    let active = supervisor.list_active().await;
    assert!(active.is_empty());
}

// ---------------------------------------------------------------------------
// subscribe
// ---------------------------------------------------------------------------

#[test]
fn subscribe_returns_a_receiver() {
    let supervisor = make_supervisor();
    let _rx = supervisor.subscribe();
    let _rx2 = supervisor.subscribe();
}

#[test]
fn subscribe_receiver_is_empty_initially() {
    let supervisor = make_supervisor();
    let mut rx = supervisor.subscribe();
    assert!(rx.try_recv().is_err());
}

// ---------------------------------------------------------------------------
// wait_any / wait_all with no children
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wait_any_returns_none_when_no_active_children() {
    let supervisor = make_supervisor();
    assert!(supervisor.wait_any().await.is_none());
}

#[tokio::test]
async fn wait_all_returns_empty_when_no_active_children() {
    let supervisor = make_supervisor();
    let results = supervisor.wait_all().await;
    assert!(results.is_empty());
}

// ---------------------------------------------------------------------------
// max_active_children cap (via spawn rejection)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_rejected_when_max_active_children_zero() {
    // max_active_children = 0 means no children can ever be spawned
    let config = SubagentSupervisorConfig {
        max_active_children: Some(0),
        ..Default::default()
    };
    let supervisor = make_supervisor_with_config(config);

    let spec = SubagentSpec {
        profile: "builtin:developer".into(),
        label: Some("test".into()),
        input: SubagentInput::Prompt {
            task: "hello".into(),
        },
        overrides: Default::default(),
    };

    let result = supervisor.spawn(spec).await;
    let err = result.err().expect("expected spawn to fail");
    assert!(err.to_string().contains("max active children"));
}

// ---------------------------------------------------------------------------
// spawn with unknown profile
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_with_unknown_profile_returns_error() {
    let supervisor = make_supervisor();
    let spec = SubagentSpec {
        profile: "nonexistent-profile".into(),
        label: None,
        input: SubagentInput::Prompt {
            task: "test".into(),
        },
        overrides: Default::default(),
    };
    let result = supervisor.spawn(spec).await;
    let err = result.err().expect("expected spawn to fail");
    assert!(err.to_string().contains("not found"));
}

// ---------------------------------------------------------------------------
// shutdown on empty supervisor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shutdown_on_empty_supervisor_is_fine() {
    let supervisor = make_supervisor();
    supervisor.shutdown().await;
    let active = supervisor.list_active().await;
    assert!(active.is_empty());
}

// ---------------------------------------------------------------------------
// abort / wait on unknown child
// ---------------------------------------------------------------------------

#[tokio::test]
async fn abort_unknown_child_returns_error() {
    let supervisor = make_supervisor();
    let result = supervisor.abort(Uuid::new_v4()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn wait_unknown_child_returns_error() {
    let supervisor = make_supervisor();
    let result = supervisor.wait(Uuid::new_v4()).await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Runtime regression coverage for child-input seeding and transcript shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_with_context_prompt_only_runs_from_seeded_history() {
    let (supervisor, requests) = make_recording_supervisor("prompt-only complete");
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: Some("prompt-only".into()),
        input: SubagentInput::Prompt {
            task: "fix the bug".into(),
        },
        overrides: Default::default(),
    };

    let handle = supervisor
        .spawn_with_context(spec, SubagentContext::default())
        .await
        .unwrap();
    let result = handle.wait().await;

    assert_eq!(result.status, SubagentStatus::Completed);
    let request_messages = captured_request_messages(&requests);
    assert_eq!(request_messages.len(), 2);
    assert_test_system_prompt(&request_messages[0]);
    assert_eq!(request_messages[1].role, Role::User);
    assert_eq!(request_messages[1].text(), "fix the bug");

    assert_eq!(result.messages.len(), 3);
    assert_test_system_prompt(&result.messages[0]);
    assert_eq!(result.messages[1].text(), "fix the bug");
    assert_eq!(result.messages[2].role, Role::Assistant);
    assert_eq!(result.messages[2].text(), "prompt-only complete");
}

#[tokio::test]
async fn spawn_with_context_snapshot_only_uses_supplied_context_without_task() {
    let (supervisor, requests) = make_recording_supervisor("snapshot-only complete");
    let context = SubagentContext {
        summary: Some("parent did X".into()),
        ..Default::default()
    };
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: Some("snapshot-only".into()),
        input: SubagentInput::Snapshot {
            mode: SnapshotMode::SummaryOnly,
        },
        overrides: Default::default(),
    };

    let handle = supervisor.spawn_with_context(spec, context).await.unwrap();
    let result = handle.wait().await;

    assert_eq!(result.status, SubagentStatus::Completed);
    let request_messages = captured_request_messages(&requests);
    assert_eq!(request_messages.len(), 3);
    assert_test_system_prompt(&request_messages[0]);
    assert_eq!(request_messages[1].role, Role::User);
    assert_eq!(
        request_messages[1].text(),
        "Parent context summary:\nparent did X"
    );
    assert_eq!(request_messages[2].role, Role::User);
    assert!(request_messages[2].text().contains("read-only snapshot"));
}

#[tokio::test]
async fn spawn_with_context_prompt_plus_snapshot_preserves_order_end_to_end() {
    let (supervisor, requests) = make_recording_supervisor("prompt+snapshot complete");
    let context = SubagentContext {
        summary: Some("summary of conversation".into()),
        selected_messages: vec![
            ModelMessage::assistant("prior answer"),
            ModelMessage::user("follow-up from parent"),
        ],
        ..Default::default()
    };
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: Some("prompt+snapshot".into()),
        input: SubagentInput::PromptWithSnapshot {
            task: "implement feature Y".into(),
            mode: SnapshotMode::SummaryOnly,
        },
        overrides: Default::default(),
    };

    let handle = supervisor.spawn_with_context(spec, context).await.unwrap();
    let result = handle.wait().await;

    assert_eq!(result.status, SubagentStatus::Completed);
    let request_messages = captured_request_messages(&requests);
    assert_eq!(
        request_messages
            .iter()
            .map(|message| (message.role, message.text()))
            .collect::<Vec<_>>(),
        vec![
            (Role::System, request_messages[0].text(),),
            (
                Role::User,
                "Parent context summary:\nsummary of conversation".into()
            ),
            (Role::Assistant, "prior answer".into()),
            (Role::User, "follow-up from parent".into()),
            (Role::User, "implement feature Y".into()),
        ]
    );
    assert_test_system_prompt(&request_messages[0]);

    assert_eq!(
        result
            .messages
            .iter()
            .map(|message| (message.role, message.text()))
            .collect::<Vec<_>>(),
        vec![
            (Role::System, result.messages[0].text()),
            (
                Role::User,
                "Parent context summary:\nsummary of conversation".into()
            ),
            (Role::Assistant, "prior answer".into()),
            (Role::User, "follow-up from parent".into()),
            (Role::User, "implement feature Y".into()),
            (Role::Assistant, "prompt+snapshot complete".into()),
        ]
    );
    assert_test_system_prompt(&result.messages[0]);
}

#[tokio::test]
async fn spawn_backward_compat_path_still_runs() {
    let (supervisor, requests) = make_recording_supervisor("spawn complete");
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: Some("backward-compat".into()),
        input: SubagentInput::Prompt {
            task: "test backward compat".into(),
        },
        overrides: Default::default(),
    };

    let handle = supervisor.spawn(spec).await.unwrap();
    let result = handle.wait().await;

    assert_eq!(result.status, SubagentStatus::Completed);
    let request_messages = captured_request_messages(&requests);
    assert_eq!(request_messages.len(), 2);
    assert_test_system_prompt(&request_messages[0]);
    assert_eq!(request_messages[1].text(), "test backward compat");
    assert_eq!(result.messages[2].text(), "spawn complete");
}

#[tokio::test]
async fn child_runtime_does_not_duplicate_system_prompt() {
    let (supervisor, requests) = make_recording_supervisor("no duplicate system");
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: Some("system-once".into()),
        input: SubagentInput::Prompt {
            task: "hello".into(),
        },
        overrides: Default::default(),
    };

    let handle = supervisor
        .spawn_with_context(spec, SubagentContext::default())
        .await
        .unwrap();
    let result = handle.wait().await;

    let request_messages = captured_request_messages(&requests);
    assert_eq!(
        request_messages
            .iter()
            .filter(|message| message.role == Role::System)
            .count(),
        1,
        "provider request should include exactly one system message"
    );
    assert_eq!(
        result
            .messages
            .iter()
            .filter(|message| message.role == Role::System)
            .count(),
        1,
        "final child transcript should include exactly one system message"
    );
}

#[cfg(feature = "agent")]
#[tokio::test]
async fn abort_while_child_waits_for_user_input_completes() {
    let supervisor = make_blocking_ask_user_supervisor();
    let mut events = supervisor.subscribe();
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: Some("abort-ask-user".into()),
        input: SubagentInput::Prompt {
            task: "use ask_user and then wait".into(),
        },
        overrides: Default::default(),
    };

    let handle = supervisor.spawn(spec).await.unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            match events.recv().await {
                Ok(SubagentEvent::AgentEvent { event, .. }) => {
                    if let AgentEvent::UserInputRequested { request } = *event {
                        assert_eq!(request.questions[0].id, "abort_unit");
                        break;
                    }
                }
                Ok(_) => {}
                Err(err) => panic!("event stream closed unexpectedly: {err}"),
            }
        }
    })
    .await
    .expect("child should request user input");

    assert!(supervisor.abort(handle.id()).await.unwrap());

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        handle.wait().await
    })
    .await
    .expect("aborted child should complete promptly");

    assert_eq!(result.status, SubagentStatus::Aborted);
}

// ---------------------------------------------------------------------------
// submit_user_input with unknown request
// ---------------------------------------------------------------------------

#[cfg(feature = "agent")]
#[tokio::test]
async fn submit_user_input_unknown_request_returns_error() {
    use crate::tools::UserInputResponse;

    let supervisor = make_supervisor();
    let response = UserInputResponse {
        request_id: Uuid::nil(),
        answers: vec![],
        canceled: false,
    };
    let result = supervisor.submit_user_input(response).await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Spawn tests that need a real provider (ignored)
// ---------------------------------------------------------------------------

/// Full spawn -> wait lifecycle test. Requires API keys and a configured
/// provider with valid credentials.
#[ignore = "requires API keys and a configured provider"]
#[tokio::test]
async fn spawn_and_wait_full_lifecycle() {
    let supervisor = make_supervisor();
    let spec = SubagentSpec {
        profile: "builtin:developer".into(),
        label: Some("live-test".into()),
        input: SubagentInput::Prompt {
            task: "say hello".into(),
        },
        overrides: Default::default(),
    };
    let handle = supervisor.spawn(spec).await.unwrap();
    let result = handle.wait().await;
    assert!(matches!(
        result.status,
        SubagentStatus::Completed | SubagentStatus::Failed
    ));
}
