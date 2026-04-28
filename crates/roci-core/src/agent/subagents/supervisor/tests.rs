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
    ModelCandidate, SnapshotMode, SubagentId, SubagentInput, SubagentSpec, SubagentStatus,
    SubagentSupervisorConfig,
};
use crate::config::RociConfig;
use crate::error::RociError as TestRociError;
use crate::models::LanguageModel;
use crate::provider::factory::ProviderFactory;
use crate::provider::{ModelProvider, ProviderRegistry};
use crate::types::{ModelMessage, Role};

use async_trait::async_trait;

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
    registry: Arc<ProviderRegistry>,
    roci_config: RociConfig,
}

impl MockLauncher {
    fn new() -> (Self, Arc<Mutex<Vec<ModelMessage>>>) {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let launcher = Self {
            captured: captured.clone(),
            registry: Arc::new(ProviderRegistry::new()),
            roci_config: RociConfig::default(),
        };
        (launcher, captured)
    }
}

#[async_trait]
impl SubagentLauncher for MockLauncher {
    async fn launch(
        &self,
        _id: SubagentId,
        model: LanguageModel,
        initial_messages: Vec<ModelMessage>,
        tools: Vec<Arc<dyn crate::tools::tool::Tool>>,
        #[cfg(feature = "agent")] coordinator: Arc<UserInputCoordinator>,
        event_sink: Option<crate::agent_loop::runner::AgentEventSink>,
    ) -> Result<LaunchedChild, crate::error::RociError> {
        // Capture the messages for test assertions.
        *self.captured.lock().await = initial_messages.clone();

        // Build a real runtime so the supervisor background task can run.
        // It will fail at LLM call (no provider configured), which the
        // supervisor handles gracefully.
        let config = {
            use crate::agent::runtime::QueueDrainMode;
            use crate::agent_loop::runner::RetryBackoffPolicy;
            use crate::resource::CompactionSettings;
            use crate::types::GenerationSettings;

            AgentConfig {
                model,
                system_prompt: None,
                tools,
                event_sink,
                approval_policy: Default::default(),
                approval_handler: None,
                #[cfg(feature = "agent")]
                user_input_coordinator: Some(coordinator),
                dynamic_tool_providers: Vec::new(),
                settings: GenerationSettings::default(),
                transform_context: None,
                convert_to_llm: None,
                before_agent_start: None,
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
                context_budget: None,
                chat: Default::default(),
            }
        };
        let runtime = AgentRuntime::new(self.registry.clone(), self.roci_config.clone(), config);
        if !initial_messages.is_empty() {
            runtime.replace_messages(initial_messages).await?;
        }
        Ok(LaunchedChild { runtime })
    }
}

fn make_supervisor_with_mock() -> (SubagentSupervisor, Arc<Mutex<Vec<ModelMessage>>>) {
    let registry = test_registry();
    let roci_config = test_roci_config();
    let base_config = make_base_config();
    let sup_config = SubagentSupervisorConfig::default();
    let profile_registry = test_profile_registry();

    let (mock, captured) = MockLauncher::new();

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
    (supervisor, captured)
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
    let (supervisor, captured) = make_supervisor_with_mock();
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

// ---------------------------------------------------------------------------
// spawn_with_context: snapshot-only mode succeeds (was broken before)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_snapshot_only_succeeds_without_caller_task() {
    let (supervisor, captured) = make_supervisor_with_mock();
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
    let (supervisor, captured) = make_supervisor_with_mock();
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
    let (supervisor, captured) = make_supervisor_with_mock();
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
    let (supervisor, captured) = make_supervisor_with_mock();
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
    let (supervisor, captured) = make_supervisor_with_mock();

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
