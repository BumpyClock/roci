use super::child_registry::{is_terminal, ChildEntry};
use super::*;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::runtime::AgentConfig;
use crate::agent::runtime::AgentRuntime;
#[cfg(feature = "agent")]
use crate::agent::runtime::UserInputCoordinator;
use crate::agent::subagents::context::materialize_context;
use crate::agent::subagents::launcher::LaunchedChild;
use crate::agent::subagents::launcher::SubagentLauncher;
use crate::agent::subagents::profiles::SubagentProfileRegistry;
use crate::agent::subagents::prompt::SubagentPromptPolicy;
use crate::agent::subagents::types::{
    ModelCandidate, SnapshotMode, SubagentId, SubagentInput, SubagentProfile, SubagentSpec,
    SubagentStatus, SubagentSupervisorConfig, ToolPolicy,
};
use crate::config::RociConfig;
use crate::error::RociError as TestRociError;
use crate::models::LanguageModel;
use crate::provider::factory::ProviderFactory;
use crate::provider::{ModelProvider, ProviderRegistry, ProviderRequest, ProviderResponse};
use crate::tools::tool::Tool;
use crate::tools::{AgentTool, AgentToolParameters};
use crate::types::{
    GenerationSettings, ModelMessage, ReasoningEffort, Role, TextStreamDelta, Usage,
};

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};

// ---------------------------------------------------------------------------
// Dummy provider factory so model resolution succeeds in tests
// ---------------------------------------------------------------------------

struct TestProviderFactory;

impl ProviderFactory for TestProviderFactory {
    fn provider_keys(&self) -> &[&str] {
        &["test"]
    }

    fn create(
        &self,
        _config: &RociConfig,
        _provider_key: &str,
        _model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, TestRociError> {
        Err(TestRociError::Configuration("test provider stub".into()))
    }
}

/// Build a ProviderRegistry with a "test" provider registered.
fn test_registry() -> Arc<ProviderRegistry> {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(TestProviderFactory));
    Arc::new(registry)
}

/// Build a RociConfig with a "test" API key set.
fn test_roci_config() -> RociConfig {
    let config = RociConfig::default();
    config.set_api_key("test", "test-key".into());
    config
}

/// Build a profile registry with a "test:dev" profile that uses the test
/// provider, so model resolution succeeds without real credentials.
fn test_profile_registry() -> SubagentProfileRegistry {
    use crate::agent::subagents::types::SubagentProfile;

    let mut registry = SubagentProfileRegistry::with_builtins();
    registry.register(SubagentProfile {
        name: "test:dev".into(),
        system_prompt: Some("You are a test sub-agent.".into()),
        models: vec![ModelCandidate {
            provider: "test".into(),
            model: "test-model".into(),
            reasoning_effort: None,
        }],
        ..Default::default()
    });
    registry
}

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
        approval_policy: Default::default(),
        approval_handler: None,
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
        chat: Default::default(),
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

// ---------------------------------------------------------------------------
// Mock launcher that captures initial_messages for assertions
// ---------------------------------------------------------------------------

struct MockLauncher {
    /// Messages received by the last `launch()` call.
    captured: Arc<Mutex<Vec<ModelMessage>>>,
    captured_config: Arc<Mutex<Option<AgentConfig>>>,
    registry: Arc<ProviderRegistry>,
    roci_config: RociConfig,
}

type CapturedMessages = Arc<Mutex<Vec<ModelMessage>>>;
type CapturedConfig = Arc<Mutex<Option<AgentConfig>>>;
type MockSupervisorParts = (SubagentSupervisor, CapturedMessages, CapturedConfig);
type MockLauncherParts = (MockLauncher, CapturedMessages, CapturedConfig);

impl MockLauncher {
    fn new() -> MockLauncherParts {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_config = Arc::new(Mutex::new(None));
        let launcher = Self {
            captured: captured.clone(),
            captured_config: captured_config.clone(),
            registry: Arc::new(ProviderRegistry::new()),
            roci_config: RociConfig::default(),
        };
        (launcher, captured, captured_config)
    }
}

#[async_trait]
impl SubagentLauncher for MockLauncher {
    async fn launch(
        &self,
        _id: SubagentId,
        initial_messages: Vec<ModelMessage>,
        config: AgentConfig,
    ) -> Result<LaunchedChild, crate::error::RociError> {
        // Capture the messages for test assertions.
        *self.captured.lock().await = initial_messages.clone();
        *self.captured_config.lock().await = Some(config.clone());

        // Build a real runtime so the supervisor background task can run.
        // It will fail at LLM call (no provider configured), which the
        // supervisor handles gracefully.
        let runtime = AgentRuntime::new(self.registry.clone(), self.roci_config.clone(), config);
        if !initial_messages.is_empty() {
            runtime.replace_messages(initial_messages).await?;
        }
        Ok(LaunchedChild { runtime })
    }
}

fn make_supervisor_with_mock() -> MockSupervisorParts {
    make_supervisor_with_mock_config(make_base_config(), test_profile_registry())
}

fn make_supervisor_with_mock_config(
    base_config: AgentConfig,
    profile_registry: SubagentProfileRegistry,
) -> MockSupervisorParts {
    let registry = test_registry();
    let roci_config = test_roci_config();
    let sup_config = SubagentSupervisorConfig::default();

    let (mock, captured, captured_config) = MockLauncher::new();

    #[cfg(feature = "agent")]
    let coordinator = base_config
        .user_input_coordinator
        .clone()
        .unwrap_or_else(|| Arc::new(UserInputCoordinator::new()));

    let (event_tx, _) = broadcast::channel(256);
    let semaphore = Arc::new(Semaphore::new(sup_config.max_concurrent));

    let supervisor = SubagentSupervisor {
        registry,
        roci_config,
        config: sup_config,
        profile_registry,
        prompt_policy: SubagentPromptPolicy::default(),
        base_config,
        launcher: Box::new(mock),
        #[cfg(feature = "agent")]
        coordinator,
        event_tx,
        children: Arc::new(Mutex::new(HashMap::new())),
        concurrency_semaphore: semaphore,
    };
    (supervisor, captured, captured_config)
}

fn dummy_tool(name: &str) -> Arc<dyn Tool> {
    Arc::new(AgentTool::new(
        name,
        "test tool",
        AgentToolParameters::empty(),
        |_args, _ctx| async { Ok(serde_json::Value::Null) },
    ))
}

fn captured_tool_names(config: &AgentConfig) -> Vec<&str> {
    config.tools.iter().map(|tool| tool.name()).collect()
}

// ---------------------------------------------------------------------------
// Basic construction
// ---------------------------------------------------------------------------

#[test]
fn supervisor_construction_with_builtins() {
    let supervisor = make_supervisor();
    assert_eq!(supervisor.config.max_concurrent, 4);
}

#[tokio::test]
async fn list_active_empty_on_fresh_supervisor() {
    let supervisor = make_supervisor();
    let active = supervisor.list_active().await;
    assert!(active.is_empty());
}

#[test]
fn subscribe_returns_receiver() {
    let supervisor = make_supervisor();
    let _rx = supervisor.subscribe();
}

// ---------------------------------------------------------------------------
// spawn_with_context: prompt-only mode passes correct messages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_prompt_only_seeds_system_and_user() {
    let (supervisor, captured, _) = make_supervisor_with_mock();
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: Some("test-prompt".into()),
        input: SubagentInput::Prompt {
            task: "fix the bug".into(),
        },
        overrides: Default::default(),
    };

    let handle = supervisor
        .spawn_with_context(spec, SubagentContext::default())
        .await
        .unwrap();

    // Give the background task a moment to launch.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs = captured.lock().await;
    assert_eq!(msgs.len(), 2, "expected [System, User(task)]");
    assert_eq!(msgs[0].role, Role::System);
    assert_eq!(msgs[1].role, Role::User);
    assert_eq!(msgs[1].text(), "fix the bug");

    // System prompt should be the composed prompt (preamble + profile).
    let preamble = SubagentPromptPolicy::default_child_preamble();
    assert!(
        msgs[0].text().starts_with(preamble),
        "system prompt must start with preamble"
    );

    // Clean up.
    handle.abort().await;
}

#[tokio::test]
async fn spawn_applies_tool_policy_and_reasoning_effort() {
    let mut base_config = make_base_config();
    base_config.tools = vec![dummy_tool("read"), dummy_tool("write"), dummy_tool("shell")];
    base_config.settings = GenerationSettings {
        temperature: Some(0.4),
        ..GenerationSettings::default()
    };

    let mut profile_registry = test_profile_registry();
    profile_registry.register(SubagentProfile {
        name: "test:scoped".into(),
        system_prompt: Some("Scoped sub-agent".into()),
        tools: ToolPolicy::Replace {
            tools: vec!["read".into(), "shell".into()],
        },
        models: vec![ModelCandidate {
            provider: "test".into(),
            model: "test-model".into(),
            reasoning_effort: Some("high".into()),
        }],
        ..Default::default()
    });
    let (supervisor, _, captured_config) =
        make_supervisor_with_mock_config(base_config, profile_registry);

    let spec = SubagentSpec {
        profile: "test:scoped".into(),
        label: None,
        input: SubagentInput::Prompt {
            task: "use scoped tools".into(),
        },
        overrides: Default::default(),
    };

    let handle = supervisor
        .spawn_with_context(spec, SubagentContext::default())
        .await
        .unwrap();

    let cfg = captured_config.lock().await.clone().unwrap();
    assert_eq!(captured_tool_names(&cfg), vec!["read", "shell"]);
    assert_eq!(cfg.settings.temperature, Some(0.4));
    assert_eq!(cfg.settings.reasoning_effort, Some(ReasoningEffort::High));

    handle.abort().await;
}

#[tokio::test]
async fn spawn_rejects_unknown_tool_policy_entry() {
    let mut base_config = make_base_config();
    base_config.tools = vec![dummy_tool("read")];
    let mut profile_registry = test_profile_registry();
    profile_registry.register(SubagentProfile {
        name: "test:missing-tool".into(),
        system_prompt: Some("Scoped sub-agent".into()),
        tools: ToolPolicy::Replace {
            tools: vec!["missing".into()],
        },
        models: vec![ModelCandidate {
            provider: "test".into(),
            model: "test-model".into(),
            reasoning_effort: None,
        }],
        ..Default::default()
    });
    let (supervisor, _, _) = make_supervisor_with_mock_config(base_config, profile_registry);

    let result = supervisor
        .spawn_with_context(
            SubagentSpec {
                profile: "test:missing-tool".into(),
                label: None,
                input: SubagentInput::Prompt { task: "x".into() },
                overrides: Default::default(),
            },
            SubagentContext::default(),
        )
        .await;
    let err = match result {
        Ok(_) => panic!("expected missing tool policy entry to fail"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("subagent tool 'missing'"));
}

// ---------------------------------------------------------------------------
// spawn_with_context: snapshot-only mode succeeds (was broken before)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_snapshot_only_succeeds_without_caller_task() {
    let (supervisor, captured, _) = make_supervisor_with_mock();
    let context = SubagentContext {
        summary: Some("parent did X".into()),
        ..Default::default()
    };
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: Some("snapshot-worker".into()),
        input: SubagentInput::Snapshot {
            mode: SnapshotMode::SummaryOnly,
        },
        overrides: Default::default(),
    };

    // This previously failed with "no task prompt in SubagentInput".
    let handle = supervisor.spawn_with_context(spec, context).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs = captured.lock().await;
    // Expect: [System, User(summary), User(continuation prompt)]
    assert_eq!(
        msgs.len(),
        3,
        "snapshot-only: [System, summary, continuation]"
    );
    assert_eq!(msgs[0].role, Role::System);
    assert!(msgs[1].text().contains("parent did X"));
    assert!(msgs[2].text().contains("read-only snapshot"));

    handle.abort().await;
}

// ---------------------------------------------------------------------------
// spawn_with_context: prompt+snapshot mode
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_prompt_with_snapshot_seeds_context_before_task() {
    let (supervisor, captured, _) = make_supervisor_with_mock();
    let context = SubagentContext {
        summary: Some("summary of conversation".into()),
        ..Default::default()
    };
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: None,
        input: SubagentInput::PromptWithSnapshot {
            task: "implement feature Y".into(),
            mode: SnapshotMode::SummaryOnly,
        },
        overrides: Default::default(),
    };

    let handle = supervisor.spawn_with_context(spec, context).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs = captured.lock().await;
    // Expect: [System, User(summary), User(task)]
    assert_eq!(msgs.len(), 3, "prompt+snapshot: [System, summary, task]");
    assert_eq!(msgs[0].role, Role::System);
    assert!(msgs[1].text().contains("summary of conversation"));
    assert_eq!(msgs[2].text(), "implement feature Y");

    handle.abort().await;
}

// ---------------------------------------------------------------------------
// System prompt applied exactly once
// ---------------------------------------------------------------------------

#[tokio::test]
async fn system_prompt_appears_exactly_once() {
    let (supervisor, captured, _) = make_supervisor_with_mock();
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: None,
        input: SubagentInput::Prompt {
            task: "hello".into(),
        },
        overrides: Default::default(),
    };

    let handle = supervisor
        .spawn_with_context(spec, SubagentContext::default())
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs = captured.lock().await;
    let system_count = msgs.iter().filter(|m| m.role == Role::System).count();
    assert_eq!(system_count, 1, "system prompt must appear exactly once");

    handle.abort().await;
}

// ---------------------------------------------------------------------------
// Backward-compat: spawn() delegates to spawn_with_context
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_without_context_uses_default() {
    let (supervisor, captured, _) = make_supervisor_with_mock();
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: None,
        input: SubagentInput::Prompt {
            task: "test backward compat".into(),
        },
        overrides: Default::default(),
    };

    let handle = supervisor.spawn(spec).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs = captured.lock().await;
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1].text(), "test backward compat");

    handle.abort().await;
}

// ---------------------------------------------------------------------------
// Full read-only snapshot mode preserves user/assistant messages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_full_snapshot_preserves_conversation() {
    let (supervisor, captured, _) = make_supervisor_with_mock();

    let parent_messages = vec![
        ModelMessage::system("parent sys"),
        ModelMessage::user("question"),
        ModelMessage::assistant("answer"),
        ModelMessage::user("follow-up"),
    ];
    let context = materialize_context(&parent_messages, &SnapshotMode::FullReadonlySnapshot, None);
    // FullReadonlySnapshot filters to user+assistant only.
    assert_eq!(context.selected_messages.len(), 3);

    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: None,
        input: SubagentInput::Snapshot {
            mode: SnapshotMode::FullReadonlySnapshot,
        },
        overrides: Default::default(),
    };

    let handle = supervisor.spawn_with_context(spec, context).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs = captured.lock().await;
    // Expect: [System, User(question), Asst(answer), User(follow-up), User(continuation)]
    assert_eq!(msgs.len(), 5);
    assert_eq!(msgs[0].role, Role::System);
    assert_eq!(msgs[1].text(), "question");
    assert_eq!(msgs[2].text(), "answer");
    assert_eq!(msgs[3].text(), "follow-up");
    assert!(msgs[4].text().contains("read-only snapshot"));

    handle.abort().await;
}

#[tokio::test]
async fn profile_timeout_aborts_child_run() {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(BlockingStreamFactory));
    let mut profile_registry = SubagentProfileRegistry::new();
    profile_registry.register(SubagentProfile {
        name: "test:timeout".into(),
        system_prompt: Some("Timeout sub-agent".into()),
        default_timeout_ms: Some(20),
        models: vec![ModelCandidate {
            provider: "test".into(),
            model: "test-model".into(),
            reasoning_effort: None,
        }],
        ..Default::default()
    });

    let supervisor = SubagentSupervisor::new(
        Arc::new(registry),
        test_roci_config(),
        make_base_config(),
        SubagentSupervisorConfig::default(),
        profile_registry,
    );
    let handle = supervisor
        .spawn_with_context(
            SubagentSpec {
                profile: "test:timeout".into(),
                label: None,
                input: SubagentInput::Prompt {
                    task: "wait forever".into(),
                },
                overrides: Default::default(),
            },
            SubagentContext::default(),
        )
        .await
        .unwrap();

    let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle.wait())
        .await
        .expect("subagent timeout should complete handle wait");

    assert_eq!(result.status, SubagentStatus::Aborted);
}

struct BlockingStreamFactory;

impl ProviderFactory for BlockingStreamFactory {
    fn provider_keys(&self) -> &[&str] {
        &["test"]
    }

    fn create(
        &self,
        _config: &RociConfig,
        provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, TestRociError> {
        Ok(Box::new(BlockingStreamProvider {
            provider_key: provider_key.to_string(),
            model_id: model_id.to_string(),
            capabilities: crate::models::capabilities::ModelCapabilities {
                supports_streaming: true,
                ..Default::default()
            },
        }))
    }
}

struct BlockingStreamProvider {
    provider_key: String,
    model_id: String,
    capabilities: crate::models::capabilities::ModelCapabilities,
}

#[async_trait]
impl ModelProvider for BlockingStreamProvider {
    fn provider_name(&self) -> &str {
        &self.provider_key
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn capabilities(&self) -> &crate::models::capabilities::ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<ProviderResponse, TestRociError> {
        Err(TestRociError::UnsupportedOperation(
            "blocking stream test provider does not generate text".into(),
        ))
    }

    async fn stream_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, TestRociError>>, TestRociError> {
        let events = stream::once(async {
            Ok(TextStreamDelta {
                text: "partial".to_string(),
                event_type: crate::types::StreamEventType::TextDelta,
                tool_call: None,
                finish_reason: None,
                usage: Some(Usage::default()),
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            })
        })
        .chain(stream::pending());
        Ok(Box::pin(events))
    }
}

// ---------------------------------------------------------------------------
// Existing tests (lifecycle, abort, wait, etc.)
// ---------------------------------------------------------------------------

#[cfg(feature = "agent")]
#[tokio::test]
async fn submit_user_input_delegates_to_coordinator() {
    use crate::tools::UserInputResponse;

    let supervisor = make_supervisor();

    // Unknown request should error
    let response = UserInputResponse {
        request_id: uuid::Uuid::nil(),
        answers: vec![],
        canceled: false,
    };
    let result = supervisor.submit_user_input(response).await;
    assert!(result.is_err());
}

#[test]
fn is_terminal_identifies_terminal_statuses() {
    assert!(is_terminal(SubagentStatus::Completed));
    assert!(is_terminal(SubagentStatus::Failed));
    assert!(is_terminal(SubagentStatus::Aborted));
    assert!(!is_terminal(SubagentStatus::Pending));
    assert!(!is_terminal(SubagentStatus::Running));
}

#[tokio::test]
async fn abort_returns_error_for_unknown_child() {
    let supervisor = make_supervisor();
    let result = supervisor.abort(Uuid::new_v4()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn wait_returns_error_for_unknown_child() {
    let supervisor = make_supervisor();
    let result = supervisor.wait(Uuid::new_v4()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn wait_any_returns_none_when_no_children() {
    let supervisor = make_supervisor();
    assert!(supervisor.wait_any().await.is_none());
}

#[tokio::test]
async fn wait_all_returns_empty_when_no_children() {
    let supervisor = make_supervisor();
    let results = supervisor.wait_all().await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn shutdown_completes_when_no_children() {
    let supervisor = make_supervisor();
    supervisor.shutdown().await;
    // Should not hang or panic
}

#[test]
fn drop_cancels_tokens_when_abort_on_drop() {
    let token = CancellationToken::new();
    let token_clone = token.clone();

    {
        let supervisor = make_supervisor();
        // Manually insert a child entry with our token
        let entry = ChildEntry {
            id: Uuid::new_v4(),
            label: None,
            profile: "test".into(),
            model: None,
            status: Arc::new(Mutex::new(SubagentStatus::Running)),
            cancel_token: token_clone,
        };
        // We need to insert without async; use try_lock since no contention
        supervisor
            .children
            .try_lock()
            .unwrap()
            .insert(entry.id, entry);
        // supervisor drops here
    }

    assert!(token.is_cancelled());
}

#[test]
fn drop_does_not_cancel_when_abort_on_drop_false() {
    let token = CancellationToken::new();
    let token_clone = token.clone();

    {
        let registry = Arc::new(ProviderRegistry::new());
        let roci_config = RociConfig::default();
        let base_config = make_base_config();
        let sup_config = SubagentSupervisorConfig {
            abort_on_drop: false,
            ..SubagentSupervisorConfig::default()
        };
        let profile_registry = SubagentProfileRegistry::with_builtins();
        let supervisor = SubagentSupervisor::new(
            registry,
            roci_config,
            base_config,
            sup_config,
            profile_registry,
        );
        let entry = ChildEntry {
            id: Uuid::new_v4(),
            label: None,
            profile: "test".into(),
            model: None,
            status: Arc::new(Mutex::new(SubagentStatus::Running)),
            cancel_token: token_clone,
        };
        supervisor
            .children
            .try_lock()
            .unwrap()
            .insert(entry.id, entry);
    }

    assert!(!token.is_cancelled());
}
