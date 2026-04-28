use super::chat::{MessageStatus, TurnStatus};
use super::support::*;
use super::*;
use crate::agent_loop::runner::BeforeAgentStartHookResult;
use crate::agent_loop::{ApprovalPolicy, RunStatus};
use crate::models::ModelCapabilities;
use crate::provider::{ModelProvider, ProviderFactory, ProviderRequest, ProviderResponse};
use crate::types::{
    FinishReason, GenerationSettings, ModelMessage, ResponseFormat, Role, StreamEventType,
    TextStreamDelta, Usage,
};
use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use futures::StreamExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use tokio::time::{timeout, Duration};

type RecordedProviderRequests = Arc<Mutex<Vec<Vec<String>>>>;

fn runtime_with_chat_provider() -> AgentRuntime {
    let registry = registry_with_streaming_provider("stub", 8, 3);
    let mut config = test_agent_config();
    config.model = "stub:chat-runtime"
        .parse()
        .expect("stub model should parse");
    AgentRuntime::new(registry, test_config(), config)
}

fn runtime_with_default_thread(thread_id: ThreadId) -> AgentRuntime {
    let registry = registry_with_streaming_provider("stub", 8, 3);
    let mut config = test_agent_config();
    config.model = "stub:chat-runtime"
        .parse()
        .expect("stub model should parse");
    config.chat.default_thread_id = Some(thread_id);
    AgentRuntime::new(registry, test_config(), config)
}

#[tokio::test]
async fn read_snapshot_initially_returns_default_thread() {
    let agent = runtime_with_chat_provider();

    let snapshot = agent.read_snapshot().await;

    assert_eq!(snapshot.schema_version, 1);
    assert_eq!(snapshot.threads.len(), 1);

    let default_thread = snapshot.threads[0].clone();
    let read_thread = agent
        .read_thread(default_thread.thread_id)
        .await
        .expect("default thread should be readable");

    assert_eq!(read_thread, default_thread);
    assert_eq!(default_thread.revision, 0);
    assert_eq!(default_thread.last_seq, 0);
    assert!(default_thread.active_turn_id.is_none());
    assert!(default_thread.turns.is_empty());
    assert!(default_thread.messages.is_empty());
    assert!(default_thread.tools.is_empty());
}

#[tokio::test]
async fn configured_default_thread_id_is_used_by_runtime_snapshot() {
    let thread_id = ThreadId::new();
    let agent = runtime_with_default_thread(thread_id);

    let snapshot = agent.read_snapshot().await;

    assert_eq!(agent.default_thread_id(), thread_id);
    assert_eq!(snapshot.threads[0].thread_id, thread_id);
    assert_eq!(
        agent
            .read_thread(thread_id)
            .await
            .expect("configured thread should be readable"),
        snapshot.threads[0]
    );
}

#[tokio::test]
async fn prompt_projects_completed_turn_and_host_ready_messages() {
    let agent = runtime_with_chat_provider();
    let before = agent.read_snapshot().await.threads[0].clone();

    let result = agent.prompt("hello").await.expect("prompt should run");

    assert_eq!(
        result.status,
        RunStatus::Completed,
        "error: {:?}",
        result.error
    );

    let snapshot = agent.read_snapshot().await;
    let thread = snapshot.threads[0].clone();
    let read_thread = agent
        .read_thread(thread.thread_id)
        .await
        .expect("thread should be readable");

    assert_eq!(read_thread, thread);
    assert!(
        thread.last_seq > before.last_seq,
        "prompt should advance chat sequence"
    );
    assert_eq!(thread.turns.len(), 1);
    assert_eq!(thread.turns[0].status, TurnStatus::Completed);
    assert!(thread.turns[0].completed_at.is_some());
    assert!(thread.active_turn_id.is_none());
    assert_eq!(thread.messages.len(), 2);
    assert_eq!(thread.messages[0].payload.role, Role::User);
    assert_eq!(thread.messages[0].payload.text(), "hello");
    assert_eq!(thread.messages[1].payload.role, Role::Assistant);
    assert_eq!(thread.messages[1].payload.text(), "hello");
    assert_eq!(thread.turns[0].turn_id.revision(), thread.revision);
    assert!(
        thread.messages[0].message_id.ordinal() < thread.messages[1].message_id.ordinal(),
        "message ids should preserve prompt/response order"
    );
    assert!(thread
        .messages
        .iter()
        .all(|message| message.status == MessageStatus::Completed));
    assert_eq!(
        thread.turns[0].message_ids,
        thread
            .messages
            .iter()
            .map(|message| message.message_id)
            .collect::<Vec<_>>()
    );
    assert!(thread
        .messages
        .iter()
        .all(|message| message.message_id.revision() == thread.revision));

    let payload_text = thread
        .messages
        .iter()
        .map(|message| message.payload.text())
        .collect::<Vec<_>>();
    assert_eq!(payload_text, ["hello".to_string(), "hello".to_string()]);
}

#[tokio::test]
async fn replace_messages_imports_exact_history_without_active_turn() {
    let agent = runtime_with_chat_provider();
    let initial = agent.read_snapshot().await.threads[0].clone();
    let history = vec![
        ModelMessage::system("stay concise"),
        ModelMessage::user("hello"),
        ModelMessage::assistant("hi"),
    ];

    agent
        .replace_messages(history.clone())
        .await
        .expect("history should import");

    let thread = agent.read_snapshot().await.threads[0].clone();

    assert_eq!(agent.messages().await, history);
    assert_eq!(
        thread
            .messages
            .iter()
            .map(|message| message.payload.clone())
            .collect::<Vec<_>>(),
        history
    );
    assert!(
        thread.revision > initial.revision,
        "history import should advance revision"
    );
    assert!(thread.active_turn_id.is_none());
    assert_eq!(thread.turns.len(), 1);
    assert!(thread
        .turns
        .iter()
        .all(|turn| turn.status == TurnStatus::Completed));
    assert!(thread
        .messages
        .iter()
        .all(|message| message.status == MessageStatus::Completed));
}

#[tokio::test]
async fn import_thread_preserves_semantic_snapshot_and_uses_model_messages_as_ledger() {
    let thread_id = ThreadId::new();
    let agent = runtime_with_default_thread(thread_id);
    let mut projector = ChatProjector::with_default_thread(
        thread_id,
        ChatRuntimeConfig {
            default_thread_id: Some(thread_id),
            ..ChatRuntimeConfig::default()
        },
    );
    let queued = projector.queue_turn(vec![ModelMessage::user("semantic prompt")]);
    projector.start_turn(queued.turn_id).expect("turn starts");
    let message = projector
        .start_message(queued.turn_id, ModelMessage::assistant("partial"))
        .expect("message starts");
    projector
        .update_reasoning(queued.turn_id, Some(message.message_id), "thinking")
        .expect("reasoning updates");
    projector
        .update_plan(queued.turn_id, "structured plan")
        .expect("plan updates");
    let semantic = projector
        .read_thread(thread_id)
        .expect("semantic thread should exist");
    let ledger = vec![
        ModelMessage::user("ledger prompt"),
        ModelMessage::assistant("ledger answer"),
    ];

    agent
        .import_thread(ImportedThread {
            thread: semantic.clone(),
            model_messages: ledger.clone(),
        })
        .await
        .expect("thread imports");

    assert_eq!(agent.read_thread(thread_id).await.unwrap(), semantic);
    assert_eq!(agent.messages().await, ledger);
}

#[tokio::test]
async fn imported_snapshot_next_ordinals_continue_from_imported_ids() {
    let thread_id = ThreadId::new();
    let agent = runtime_with_default_thread(thread_id);
    let mut projector = ChatProjector::with_default_thread(
        thread_id,
        ChatRuntimeConfig {
            default_thread_id: Some(thread_id),
            ..ChatRuntimeConfig::default()
        },
    );
    let semantic = projector
        .bootstrap_thread(vec![ModelMessage::user("imported")])
        .expect("history bootstraps");
    let imported_turn_ordinal = semantic.turns[0].turn_id.ordinal();
    let imported_message_ordinal = semantic.messages[0].message_id.ordinal();
    agent
        .import_thread(ImportedThread {
            thread: semantic,
            model_messages: vec![ModelMessage::user("ledger")],
        })
        .await
        .expect("thread imports");

    let turn_id = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("next")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: None,
        })
        .await
        .expect("turn queues");
    wait_for_turn_status(&agent, turn_id, TurnStatus::Completed).await;
    let thread = agent.read_thread(thread_id).await.unwrap();

    assert_eq!(turn_id.ordinal(), imported_turn_ordinal + 1);
    assert!(thread
        .messages
        .iter()
        .any(|message| message.message_id.ordinal() == imported_message_ordinal + 1));
}

#[tokio::test]
async fn reset_clears_chat_messages_and_invalidates_prior_ids() {
    let agent = runtime_with_chat_provider();
    agent
        .replace_messages(vec![ModelMessage::user("old")])
        .await
        .expect("history should import");
    let before = agent.read_snapshot().await.threads[0].clone();
    let old_message_id = before.messages[0].message_id;

    agent.reset().await;

    let after = agent.read_snapshot().await.threads[0].clone();

    assert!(agent.messages().await.is_empty());
    assert!(after.messages.is_empty());
    assert!(after.turns.is_empty());
    assert!(after.tools.is_empty());
    assert!(after.active_turn_id.is_none());
    assert!(
        after.revision > before.revision || after.last_seq > before.last_seq,
        "reset should make prior ids stale-compatible"
    );
    assert!(
        after.revision != old_message_id.revision() || after.last_seq > before.last_seq,
        "old message ids should not look current after reset"
    );
}

#[tokio::test]
async fn queued_cancel_prevents_provider_call_and_emits_turn_canceled() {
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with_counting_blocking_provider("stub", provider_calls.clone());
    let mut config = test_agent_config();
    config.model = "stub:blocking".parse().expect("stub model should parse");
    let agent = Arc::new(AgentRuntime::new(registry, test_config(), config));
    let mut sub = agent.subscribe(None).await;

    let running_agent = Arc::clone(&agent);
    let running = tokio::spawn(async move { running_agent.prompt("first").await });
    wait_for_provider_call(&provider_calls).await;

    let queued = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("second")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: None,
        })
        .await
        .expect("second turn queues");
    agent
        .cancel_turn(queued)
        .await
        .expect("queued turn cancels");
    assert_eq!(provider_calls.load(Ordering::SeqCst), 1);
    assert!(events_until_canceled(&mut sub, queued).await);

    agent.abort().await;
    let _ = timeout(Duration::from_secs(2), running)
        .await
        .expect("running prompt should finish")
        .expect("prompt task should not panic");
    agent.wait_for_idle().await;
    assert_eq!(provider_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn canceling_queued_turn_does_not_abort_running_turn() {
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with_counting_blocking_provider("stub", provider_calls.clone());
    let mut config = test_agent_config();
    config.model = "stub:blocking".parse().expect("stub model should parse");
    let agent = Arc::new(AgentRuntime::new(registry, test_config(), config));
    let mut sub = agent.subscribe(None).await;

    let running_agent = Arc::clone(&agent);
    let mut running = Box::pin(tokio::spawn(
        async move { running_agent.prompt("first").await },
    ));
    wait_for_provider_call(&provider_calls).await;

    let queued = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("second")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: None,
        })
        .await
        .expect("second turn queues");
    agent
        .cancel_turn(queued)
        .await
        .expect("queued turn cancels");

    assert_eq!(agent.state().await, AgentState::Running);
    assert!(
        timeout(Duration::from_millis(50), &mut running)
            .await
            .is_err(),
        "running prompt should remain active after queued cancel"
    );
    assert!(events_until_canceled(&mut sub, queued).await);
    assert!(agent.abort().await);
    let _ = timeout(Duration::from_secs(2), running)
        .await
        .expect("running prompt should finish")
        .expect("prompt task should not panic");
}

#[tokio::test]
async fn idle_mutations_reject_while_turn_is_queued() {
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with_counting_blocking_provider("stub", provider_calls.clone());
    let mut config = test_agent_config();
    config.model = "stub:blocking".parse().expect("stub model should parse");
    let agent = AgentRuntime::new(registry, test_config(), config);

    let turn_id = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("queued")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: None,
        })
        .await
        .expect("turn queues");

    let result = agent
        .replace_messages(vec![ModelMessage::user("rewrite")])
        .await;
    assert!(matches!(result, Err(RociError::InvalidState(_))));

    agent.cancel_turn(turn_id).await.expect("turn cancels");
    agent.wait_for_idle().await;
}

#[tokio::test]
async fn reset_after_enqueue_cancels_queue_without_stranding_runtime() {
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let registry = registry_with_counting_blocking_provider("stub", provider_calls);
    let mut config = test_agent_config();
    config.model = "stub:blocking".parse().expect("stub model should parse");
    let agent = AgentRuntime::new(registry, test_config(), config);

    agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("queued")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: None,
        })
        .await
        .expect("turn queues");

    timeout(Duration::from_secs(2), agent.reset())
        .await
        .expect("reset should not strand runtime");
    assert_eq!(agent.state().await, AgentState::Idle);
    assert!(agent.messages().await.is_empty());
}

#[tokio::test]
async fn queued_turns_run_fifo() {
    let (registry, requests) = registry_with_request_recording_provider("stub", "ok");
    let mut config = test_agent_config();
    config.model = "stub:fifo".parse().expect("stub model should parse");
    let agent = AgentRuntime::new(registry, test_config(), config);

    let first = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("first")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: None,
        })
        .await
        .expect("first turn queues");
    let second = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("second")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: None,
        })
        .await
        .expect("second turn queues");

    wait_for_turn_status(&agent, second, TurnStatus::Completed).await;
    let recorded = requests.lock().expect("requests lock").clone();

    assert_eq!(first.ordinal() + 1, second.ordinal());
    assert_eq!(recorded[0], vec!["first".to_string()]);
    assert_eq!(
        recorded[1]
            .iter()
            .filter(|text| text.as_str() == "first" || text.as_str() == "second")
            .cloned()
            .collect::<Vec<_>>(),
        vec!["first".to_string(), "second".to_string()]
    );
}

#[tokio::test]
async fn pre_start_cancel_does_not_commit_canceled_prompt_to_provider_ledger() {
    let (registry, requests) = registry_with_request_recording_provider("stub", "ok");
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let entered_for_hook = entered.clone();
    let release_for_hook = release.clone();
    let hook_calls_for_hook = hook_calls.clone();
    let mut config = test_agent_config();
    config.model = "stub:pre-start".parse().expect("stub model should parse");
    config.before_agent_start = Some(Arc::new(move |_payload| {
        let entered = entered_for_hook.clone();
        let release = release_for_hook.clone();
        let hook_calls = hook_calls_for_hook.clone();
        Box::pin(async move {
            if hook_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                entered.notify_waiters();
                release.notified().await;
            }
            Ok(BeforeAgentStartHookResult::Continue)
        })
    }));
    let agent = AgentRuntime::new(registry, test_config(), config);

    let canceled = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("canceled prompt")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: None,
        })
        .await
        .expect("turn queues");
    timeout(Duration::from_secs(2), entered.notified())
        .await
        .expect("hook should be entered");
    agent.cancel_turn(canceled).await.expect("turn cancels");
    release.notify_waiters();
    wait_for_turn_status(&agent, canceled, TurnStatus::Canceled).await;

    let next = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("next prompt")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: None,
        })
        .await
        .expect("next turn queues");
    wait_for_turn_status(&agent, next, TurnStatus::Completed).await;

    let recorded = requests.lock().expect("requests lock").clone();
    assert_eq!(recorded, vec![vec!["next prompt".to_string()]]);
}

#[tokio::test]
async fn queued_turns_replay_through_runtime_subscription() {
    let agent = runtime_with_chat_provider();
    let thread_id = agent.default_thread_id();
    let cursor = RuntimeCursor::new(thread_id, 0);

    let turn_id = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("queued replay")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: None,
        })
        .await
        .expect("turn queues");
    wait_for_turn_status(&agent, turn_id, TurnStatus::Completed).await;

    let replay = agent
        .subscribe(Some(cursor))
        .await
        .replay()
        .expect("queued turn should replay");

    assert!(replay.iter().any(|event| matches!(
        event.payload,
        AgentRuntimeEventPayload::TurnQueued { ref turn } if turn.turn_id == turn_id
    )));
}

#[tokio::test]
async fn idle_settings_mutator_and_per_turn_override_freeze_effective_settings() {
    let seen_settings = Arc::new(Mutex::new(Vec::new()));
    let registry = registry_with_recording_provider("stub", seen_settings.clone(), None);
    let mut config = test_agent_config();
    config.model = "stub:settings".parse().expect("stub model should parse");
    let agent = AgentRuntime::new(registry, test_config(), config);

    agent
        .set_generation_settings(GenerationSettings {
            max_tokens: Some(11),
            ..GenerationSettings::default()
        })
        .await
        .expect("settings update");
    agent.prompt("default").await.expect("prompt should run");
    let override_turn = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("override")],
            generation_settings: Some(GenerationSettings {
                max_tokens: Some(22),
                ..GenerationSettings::default()
            }),
            approval_policy: Some(ApprovalPolicy::Never),
            collaboration_mode: None,
        })
        .await
        .expect("turn queues");
    wait_for_turn_status(&agent, override_turn, TurnStatus::Completed).await;

    let max_tokens = seen_settings
        .lock()
        .expect("settings lock")
        .iter()
        .map(|settings| settings.max_tokens)
        .collect::<Vec<_>>();
    assert_eq!(max_tokens, [Some(11), Some(22)]);
}

#[tokio::test]
async fn plan_mode_emits_plan_updated_from_structured_response() {
    let seen_settings = Arc::new(Mutex::new(Vec::new()));
    let registry = registry_with_recording_provider(
        "stub",
        seen_settings.clone(),
        Some(r#"{"steps":["inspect","edit","verify"]}"#.to_string()),
    );
    let mut config = test_agent_config();
    config.model = "stub:plan".parse().expect("stub model should parse");
    let agent = AgentRuntime::new(registry, test_config(), config);
    let mut sub = agent.subscribe(None).await;

    let turn_id = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("make a plan")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: Some(CollaborationMode::Plan),
        })
        .await
        .expect("plan turn queues");
    wait_for_turn_status(&agent, turn_id, TurnStatus::Completed).await;

    let thread = agent.read_thread(agent.default_thread_id()).await.unwrap();
    assert_eq!(thread.plans[0].turn_id, turn_id);
    assert_eq!(thread.plans[0].plan, "1. inspect\n2. edit\n3. verify");
    assert!(events_until_plan(&mut sub, turn_id).await);
    assert!(matches!(
        seen_settings.lock().expect("settings lock")[0].response_format,
        Some(ResponseFormat::JsonSchema { .. })
    ));
}

#[tokio::test]
async fn plan_mode_malformed_response_marks_turn_failed_without_plan_update() {
    let seen_settings = Arc::new(Mutex::new(Vec::new()));
    let registry = registry_with_recording_provider(
        "stub",
        seen_settings,
        Some("plain prose plan".to_string()),
    );
    let mut config = test_agent_config();
    config.model = "stub:plan".parse().expect("stub model should parse");
    let agent = AgentRuntime::new(registry, test_config(), config);

    let turn_id = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("make a plan")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: Some(CollaborationMode::Plan),
        })
        .await
        .expect("plan turn queues");
    wait_for_turn_status(&agent, turn_id, TurnStatus::Failed).await;

    let thread = agent.read_thread(agent.default_thread_id()).await.unwrap();
    let turn = thread
        .turns
        .iter()
        .find(|turn| turn.turn_id == turn_id)
        .expect("turn should exist");
    assert!(turn
        .error
        .as_ref()
        .is_some_and(|error| error.contains("structured plan contract")));
    assert!(thread.plans.is_empty());
}

async fn events_until_canceled(sub: &mut RuntimeSubscription, turn_id: TurnId) -> bool {
    for _ in 0..16 {
        let event = timeout(Duration::from_secs(2), sub.recv())
            .await
            .expect("event should arrive")
            .expect("subscription should succeed");
        if matches!(
            event.payload,
            AgentRuntimeEventPayload::TurnCanceled { ref turn } if turn.turn_id == turn_id
        ) {
            return true;
        }
    }
    false
}

async fn wait_for_turn_status(agent: &AgentRuntime, turn_id: TurnId, status: TurnStatus) {
    timeout(Duration::from_secs(2), async {
        loop {
            let thread = agent
                .read_thread(turn_id.thread_id())
                .await
                .expect("thread should exist");
            if thread
                .turns
                .iter()
                .any(|turn| turn.turn_id == turn_id && turn.status == status)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("turn should reach expected status");
}

async fn wait_for_provider_call(provider_calls: &AtomicUsize) {
    timeout(Duration::from_secs(2), async {
        while provider_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("provider should be called");
}

async fn events_until_plan(sub: &mut RuntimeSubscription, turn_id: TurnId) -> bool {
    for _ in 0..16 {
        let event = timeout(Duration::from_secs(2), sub.recv())
            .await
            .expect("event should arrive")
            .expect("subscription should succeed");
        if matches!(
            event.payload,
            AgentRuntimeEventPayload::PlanUpdated { ref plan } if plan.turn_id == turn_id
        ) {
            return true;
        }
    }
    false
}

fn registry_with_counting_blocking_provider(
    provider_key: &'static str,
    calls: Arc<AtomicUsize>,
) -> Arc<ProviderRegistry> {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(CountingBlockingFactory {
        provider_key,
        calls,
    }));
    Arc::new(registry)
}

fn registry_with_recording_provider(
    provider_key: &'static str,
    settings: Arc<Mutex<Vec<GenerationSettings>>>,
    response: Option<String>,
) -> Arc<ProviderRegistry> {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(RecordingFactory {
        provider_key,
        settings,
        requests,
        response: response.unwrap_or_else(|| "hello".to_string()),
    }));
    Arc::new(registry)
}

fn registry_with_request_recording_provider(
    provider_key: &'static str,
    response: &str,
) -> (Arc<ProviderRegistry>, RecordedProviderRequests) {
    let settings = Arc::new(Mutex::new(Vec::new()));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(RecordingFactory {
        provider_key,
        settings,
        requests: requests.clone(),
        response: response.to_string(),
    }));
    (Arc::new(registry), requests)
}

struct CountingBlockingFactory {
    provider_key: &'static str,
    calls: Arc<AtomicUsize>,
}

impl ProviderFactory for CountingBlockingFactory {
    fn provider_keys(&self) -> &[&str] {
        std::slice::from_ref(&self.provider_key)
    }

    fn create(
        &self,
        _config: &RociConfig,
        provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        Ok(Box::new(CountingBlockingProvider {
            provider_key: provider_key.to_string(),
            model_id: model_id.to_string(),
            calls: self.calls.clone(),
        }))
    }
}

struct CountingBlockingProvider {
    provider_key: String,
    model_id: String,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ModelProvider for CountingBlockingProvider {
    fn provider_name(&self) -> &str {
        &self.provider_key
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn capabilities(&self) -> &ModelCapabilities {
        static CAPABILITIES: std::sync::OnceLock<ModelCapabilities> = std::sync::OnceLock::new();
        CAPABILITIES.get_or_init(ModelCapabilities::default)
    }

    async fn generate_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        Err(RociError::UnsupportedOperation(
            "stream-only blocking test provider".to_string(),
        ))
    }

    async fn stream_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let events = stream::once(async {
            Ok(TextStreamDelta {
                text: "partial".to_string(),
                event_type: StreamEventType::TextDelta,
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

struct RecordingFactory {
    provider_key: &'static str,
    settings: Arc<Mutex<Vec<GenerationSettings>>>,
    requests: Arc<Mutex<Vec<Vec<String>>>>,
    response: String,
}

impl ProviderFactory for RecordingFactory {
    fn provider_keys(&self) -> &[&str] {
        std::slice::from_ref(&self.provider_key)
    }

    fn create(
        &self,
        _config: &RociConfig,
        provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        Ok(Box::new(RecordingProvider {
            provider_key: provider_key.to_string(),
            model_id: model_id.to_string(),
            settings: self.settings.clone(),
            requests: self.requests.clone(),
            response: self.response.clone(),
        }))
    }
}

struct RecordingProvider {
    provider_key: String,
    model_id: String,
    settings: Arc<Mutex<Vec<GenerationSettings>>>,
    requests: Arc<Mutex<Vec<Vec<String>>>>,
    response: String,
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
        static CAPABILITIES: std::sync::OnceLock<ModelCapabilities> = std::sync::OnceLock::new();
        CAPABILITIES.get_or_init(ModelCapabilities::default)
    }

    async fn generate_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        Err(RociError::UnsupportedOperation(
            "stream-only recording test provider".to_string(),
        ))
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        self.settings
            .lock()
            .expect("settings lock")
            .push(request.settings.clone());
        self.requests
            .lock()
            .expect("requests lock")
            .push(request.messages.iter().map(ModelMessage::text).collect());
        let response = self.response.clone();
        let events: Vec<Result<TextStreamDelta, RociError>> = vec![
            Ok(TextStreamDelta {
                text: response,
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
                usage: Some(Usage::default()),
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            }),
        ];
        Ok(Box::pin(stream::iter(events)))
    }
}
