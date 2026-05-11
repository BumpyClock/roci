//! High-level Agent runtime wrapping the agent loop.
//!
//! Provides the public high-level Agent runtime API surface:
//! - [`Agent::prompt`] — start a new conversation
//! - [`Agent::continue_run`] — continue with additional context
//! - [`Agent::continue_without_input`] — continue without appending a new user message
//! - [`Agent::steer`] — interrupt tool execution with a message
//! - [`Agent::follow_up`] — queue a message after natural completion
//! - [`Agent::abort`] — cancel the current run
//! - [`Agent::reset`] — clear conversation and state
//! - [`Agent::wait_for_idle`] — block until the agent finishes
//! - Runtime mutators (`set/clear` system prompt, `replace_messages`, `set_tools`) while idle
//! - Fine-grained queue controls (`clear_*_queue`, `clear_all_queues`, `has_queued_messages`)

#[cfg(test)]
use std::collections::HashMap;
use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{broadcast, mpsc, oneshot, watch, Mutex, Notify};

pub mod chat;
mod config;
mod events;
mod lifecycle;
mod mutations;
mod run_loop;
mod state;
mod summary;
mod types;

#[cfg(feature = "agent")]
pub use crate::human_interaction::HumanInteractionCoordinator;

pub use self::chat::*;
pub use self::config::AgentConfig;
#[cfg(feature = "agent")]
pub use self::config::AgentSubagentConfig;
#[cfg(test)]
use self::types::drain_queue;
pub use self::types::{
    AgentSnapshot, AgentState, GetApiKeyFn, QueueDrainMode, SessionBeforeCompactHook,
    SessionBeforeCompactOutcome, SessionBeforeCompactPayload, SessionBeforeTreeHook,
    SessionBeforeTreeOutcome, SessionBeforeTreePayload, SessionCompactionOverride,
    SummaryPreparationData,
};

#[cfg(test)]
use crate::agent::message::AgentMessageExt;
#[cfg(test)]
use crate::agent_loop::compaction::{serialize_pi_mono_summary, PiMonoSummary};
#[cfg(test)]
use crate::agent_loop::runner::BeforeAgentStartHookResult;
use crate::agent_loop::ApprovalPolicy;
use crate::agent_loop::LoopRunner;
#[cfg(test)]
use crate::agent_loop::RunStatus;
use crate::config::RociConfig;
#[cfg(test)]
use crate::context::estimate_message_tokens;
use crate::error::RociError;
use crate::models::{LanguageModel, ModelCandidates};
use crate::provider::ProviderRegistry;
#[cfg(test)]
use crate::provider::ProviderRequest;
#[cfg(test)]
use crate::resource::{BranchSummarySettings, CompactionSettings};
use crate::session::{
    LocalProviderLedger, LocalSessionFs, LocalSessionResources, SessionLease, SessionResumeState,
};
use crate::tools::dynamic::DynamicToolProvider;
use crate::tools::tool::Tool;
use crate::tools::SandboxProvider;
#[cfg(test)]
use crate::types::Role;
use crate::types::{GenerationSettings, ModelMessage, Usage};

/// High-level agent runtime wrapping [`LoopRunner`].
///
/// Manages conversation history, steering/follow-up queues, and run lifecycle.
/// All public methods are `&self` — interior mutability via `Arc<Mutex<_>>` and
/// `watch` channels lets multiple tasks share a single `AgentRuntime` handle.
///
/// # Example
///
/// ```ignore
/// use roci_core::attachments::{Attachment, PromptInput};
///
/// let agent = AgentRuntime::new(registry, roci_config, config);
/// let result = agent.prompt("Hello").await?;
/// let input = PromptInput::new("Tell me more")
///     .with_attachment(Attachment::selection("selected context"));
/// let result = agent.continue_run(input).await?;
/// let result = agent.continue_without_input().await?;
/// agent.reset().await;
/// ```
#[derive(Clone)]
pub struct AgentRuntime {
    config: AgentConfig,
    runner: LoopRunner,
    roci_config: RociConfig,
    registry: Arc<ProviderRegistry>,
    state: Arc<Mutex<AgentState>>,
    state_tx: watch::Sender<AgentState>,
    state_rx: watch::Receiver<AgentState>,
    candidates: Arc<Mutex<Vec<LanguageModel>>>,
    generation_settings: Arc<Mutex<GenerationSettings>>,
    approval_policy: Arc<Mutex<ApprovalPolicy>>,
    system_prompt: Arc<Mutex<Option<String>>>,
    tools: Arc<Mutex<Vec<Arc<dyn Tool>>>>,
    dynamic_tool_providers: Arc<Mutex<Vec<Arc<dyn DynamicToolProvider>>>>,
    messages: Arc<Mutex<Vec<ModelMessage>>>,
    steering_queue: Arc<Mutex<Vec<ModelMessage>>>,
    follow_up_queue: Arc<Mutex<Vec<ModelMessage>>>,
    active_abort_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    queued_turn_state: Arc<Mutex<QueuedTurnState>>,
    queued_turn_count: Arc<StdMutex<usize>>,
    queued_turn_notify: Arc<Notify>,
    idle_notify: Arc<Notify>,
    turn_index: Arc<Mutex<usize>>,
    is_streaming: Arc<Mutex<bool>>,
    last_error: Arc<Mutex<Option<String>>>,
    snapshot_tx: watch::Sender<AgentSnapshot>,
    snapshot_rx: watch::Receiver<AgentSnapshot>,
    chat_projector: Arc<StdMutex<ChatProjector>>,
    runtime_event_tx: broadcast::Sender<AgentRuntimeEvent>,
    runtime_event_store: Arc<dyn AgentRuntimeEventStore>,
    runtime_event_send_lock: Arc<StdMutex<()>>,
    runtime_event_publish_tx: mpsc::UnboundedSender<RuntimeEventPublishRequest>,
    runtime_event_publish_rx:
        Arc<Mutex<Option<mpsc::UnboundedReceiver<RuntimeEventPublishRequest>>>>,
    #[cfg(feature = "agent")]
    subagent_controller: Option<Arc<crate::agent::subagents::SubagentRoutingController>>,
    session_config: Option<crate::session::SessionConfig>,
    session_fs: Option<Arc<LocalSessionFs>>,
    session_resources: Option<Arc<LocalSessionResources>>,
    provider_ledger: Option<Arc<LocalProviderLedger>>,
    persisted_provider_message_count: Arc<Mutex<usize>>,
    session_lease: Option<Arc<SessionLease>>,
    sandbox_provider: Option<Arc<dyn SandboxProvider>>,
    /// Persistent session usage ledger. Accumulates across runs, cleared on reset.
    session_usage: Arc<Mutex<Usage>>,
    #[cfg(feature = "agent")]
    human_interaction_coordinator: Arc<HumanInteractionCoordinator>,
    #[cfg(feature = "agent")]
    tool_permission_session_approvals: crate::human_interaction::ToolPermissionSessionApprovals,
}

#[derive(Debug)]
struct QueuedTurn {
    turn_id: TurnId,
    messages: Vec<ModelMessage>,
    options: run_loop::TurnRunOptions,
}

#[derive(Debug, Default)]
struct QueuedTurnState {
    turns: VecDeque<QueuedTurn>,
    worker_active: bool,
}

pub(super) struct RuntimeEventPublishRequest {
    pub events: Vec<AgentRuntimeEvent>,
    pub ack_tx: Option<oneshot::Sender<Result<Vec<RuntimeCursor>, AgentRuntimeError>>>,
    pub error_slot: Option<Arc<StdMutex<Option<AgentRuntimeError>>>>,
}

impl AgentRuntime {
    /// Create a new agent runtime with the given configuration.
    pub fn new(
        registry: Arc<ProviderRegistry>,
        roci_config: RociConfig,
        config: AgentConfig,
    ) -> Self {
        Self::new_inner(registry, roci_config, config).expect(
            "AgentRuntime::new failed; use AgentRuntime::try_new for fallible session setup",
        )
    }

    /// Try to create a new agent runtime with fallible session setup.
    pub fn try_new(
        registry: Arc<ProviderRegistry>,
        roci_config: RociConfig,
        config: AgentConfig,
    ) -> Result<Self, RociError> {
        Self::new_inner(registry, roci_config, config)
    }

    fn new_inner(
        registry: Arc<ProviderRegistry>,
        roci_config: RociConfig,
        config: AgentConfig,
    ) -> Result<Self, RociError> {
        let runner = LoopRunner::with_registry(roci_config.clone(), registry.clone());
        let candidates = Arc::new(Mutex::new(
            ModelCandidates::new(config.candidates.clone())?.into_vec(),
        ));
        let generation_settings = Arc::new(Mutex::new(config.settings.clone()));
        let approval_policy = Arc::new(Mutex::new(config.approval_policy));
        let system_prompt = Arc::new(Mutex::new(config.system_prompt.clone()));
        let tools = Arc::new(Mutex::new(config.tools.clone()));
        let dynamic_tool_providers = Arc::new(Mutex::new(config.dynamic_tool_providers.clone()));
        let replay_capacity = normalized_replay_capacity(config.chat.replay_capacity);
        let session_conventions = config.session.as_ref().map(|session| session.conventions());
        let session_resources = None;
        let session_fs = None;
        let runtime_event_store: Arc<dyn AgentRuntimeEventStore> = match session_conventions {
            Some(_) => config.chat.event_store.clone().ok_or_else(|| {
                RociError::InvalidState(
                    "session runtime event store must be prepared by LocalSessionStore".to_string(),
                )
            })?,
            None => config.chat.event_store.clone().unwrap_or_else(|| {
                Arc::new(InMemoryAgentRuntimeEventStore::with_replay_capacity(
                    replay_capacity,
                ))
            }),
        };
        let session_config = config.session.clone();
        let sandbox_provider = config.sandbox_provider.clone();
        let (runtime_event_tx, _) = broadcast::channel(replay_capacity.get());
        let (runtime_event_publish_tx, runtime_event_publish_rx) =
            mpsc::unbounded_channel::<RuntimeEventPublishRequest>();
        let runtime_event_send_lock = Arc::new(StdMutex::new(()));
        let chat_projector = Arc::new(StdMutex::new(ChatProjector::new(config.chat.clone())));
        let (state_tx, state_rx) = watch::channel(AgentState::Idle);
        let initial_snapshot = AgentSnapshot {
            state: AgentState::Idle,
            turn_index: 0,
            message_count: 0,
            is_streaming: false,
            last_error: None,
        };
        let (snapshot_tx, snapshot_rx) = watch::channel(initial_snapshot);
        #[cfg(feature = "agent")]
        let human_interaction_coordinator = config
            .human_interaction_coordinator
            .clone()
            .unwrap_or_else(|| Arc::new(HumanInteractionCoordinator::new()));
        #[cfg(feature = "agent")]
        let tool_permission_session_approvals =
            Arc::new(Mutex::new(std::collections::HashSet::new()));
        #[cfg(feature = "agent")]
        let subagent_controller = config.subagents.as_ref().and_then(|subagents| {
            if !subagents.enabled {
                return None;
            }
            let mut base_config = config.clone();
            base_config.subagents = None;
            Some(Arc::new(
                crate::agent::subagents::SubagentRoutingController::new(
                    registry.clone(),
                    roci_config.clone(),
                    base_config,
                    subagents.supervisor.clone(),
                    subagents.profiles.clone(),
                ),
            ))
        });
        #[cfg(feature = "agent")]
        if let Some(controller) = &subagent_controller {
            spawn_subagent_runtime_event_bridge(
                controller.clone(),
                chat_projector.clone(),
                runtime_event_publish_tx.clone(),
                runtime_event_send_lock.clone(),
            );
        }
        Ok(Self {
            config,
            runner,
            roci_config,
            registry,
            state: Arc::new(Mutex::new(AgentState::Idle)),
            state_tx,
            state_rx,
            candidates,
            generation_settings,
            approval_policy,
            system_prompt,
            tools,
            dynamic_tool_providers,
            messages: Arc::new(Mutex::new(Vec::new())),
            steering_queue: Arc::new(Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            active_abort_tx: Arc::new(Mutex::new(None)),
            queued_turn_state: Arc::new(Mutex::new(QueuedTurnState::default())),
            queued_turn_count: Arc::new(StdMutex::new(0)),
            queued_turn_notify: Arc::new(Notify::new()),
            idle_notify: Arc::new(Notify::new()),
            turn_index: Arc::new(Mutex::new(0)),
            is_streaming: Arc::new(Mutex::new(false)),
            last_error: Arc::new(Mutex::new(None)),
            snapshot_tx,
            snapshot_rx,
            chat_projector,
            runtime_event_tx,
            runtime_event_store,
            runtime_event_send_lock,
            runtime_event_publish_tx,
            runtime_event_publish_rx: Arc::new(Mutex::new(Some(runtime_event_publish_rx))),
            #[cfg(feature = "agent")]
            subagent_controller,
            session_config,
            session_fs,
            session_resources,
            provider_ledger: None,
            persisted_provider_message_count: Arc::new(Mutex::new(0)),
            session_lease: None,
            sandbox_provider,
            session_usage: Arc::new(Mutex::new(Usage::default())),
            #[cfg(feature = "agent")]
            human_interaction_coordinator,
            #[cfg(feature = "agent")]
            tool_permission_session_approvals,
        })
    }

    /// Resume an agent runtime from prepared local session state.
    ///
    /// # Errors
    ///
    /// Returns an error when config mismatches the prepared session state or
    /// runtime construction fails.
    pub async fn resume_session(
        registry: Arc<ProviderRegistry>,
        roci_config: RociConfig,
        mut config: AgentConfig,
        state: SessionResumeState,
    ) -> Result<Self, RociError> {
        if state.session_config.id != state.metadata.id {
            return Err(RociError::InvalidState(
                "resume state session id does not match metadata id".to_string(),
            ));
        }
        if let Some(existing) = &config.session {
            if existing != &state.session_config {
                return Err(RociError::InvalidState(
                    "resume session config does not match resume state".to_string(),
                ));
            }
        }
        if let Some(thread_id) = config.chat.default_thread_id {
            if thread_id != state.default_thread_id {
                return Err(RociError::InvalidState(
                    "resume default thread id does not match resume state".to_string(),
                ));
            }
        }

        config.session = Some(state.session_config.clone());
        config.chat.default_thread_id = Some(state.default_thread_id);
        config.chat.event_store = Some(Arc::new(
            JsonlAgentRuntimeEventStore::open(state.session_config.conventions().events_file())
                .map_err(Self::map_chat_projection_error)?,
        ));
        let mut agent = Self::try_new(registry, roci_config, config)?;
        agent.session_fs = Some(Arc::new(
            LocalSessionFs::open_existing_with_conventions(state.session_config.conventions())
                .map_err(|err| RociError::InvalidState(err.to_string()))?,
        ));
        agent.session_resources = Some(Arc::new(
            LocalSessionResources::open_existing_with_conventions(
                state.session_config.conventions(),
            )
            .map_err(|err| RociError::InvalidState(err.to_string()))?,
        ));
        let persisted_count = state.model_messages.len();
        agent
            .import_runtime_snapshot(state.runtime, state.model_messages)
            .await?;
        agent.provider_ledger = Some(Arc::new(
            LocalProviderLedger::open(state.session_config.conventions().provider_ledger_file())
                .map_err(|err| RociError::InvalidState(err.to_string()))?,
        ));
        *agent.persisted_provider_message_count.lock().await = persisted_count;
        agent.session_lease = Some(state.lease);
        Ok(agent)
    }

    async fn import_runtime_snapshot(
        &self,
        snapshot: RuntimeSnapshot,
        model_messages: Vec<ModelMessage>,
    ) -> Result<(), RociError> {
        {
            let mut projector = self
                .chat_projector
                .lock()
                .map_err(|_| RociError::InvalidState("chat projector lock poisoned".into()))?;
            for thread in snapshot.threads {
                projector
                    .import_thread(thread)
                    .map_err(Self::map_chat_projection_error)?;
            }
        }
        *self.messages.lock().await = model_messages;
        self.broadcast_snapshot().await;
        Ok(())
    }

    /// Submit a user input response.
    ///
    /// This is called by the CLI/host when a user responds to a human interaction event.
    /// The response will be routed to the waiting tool execution.
    #[cfg(feature = "agent")]
    pub async fn submit_user_input(
        &self,
        response: crate::tools::UserInputResponse,
    ) -> Result<(), crate::tools::UnknownUserInputRequest> {
        self.human_interaction_coordinator
            .submit_user_input_response(response)
            .await
    }

    /// Submit a tool permission decision.
    #[cfg(feature = "agent")]
    pub async fn submit_tool_permission(
        &self,
        request_id: crate::human_interaction::HumanInteractionRequestId,
        decision: crate::human_interaction::ToolPermissionDecision,
    ) -> Result<(), crate::human_interaction::UnknownHumanInteractionRequest> {
        self.human_interaction_coordinator
            .submit_tool_permission_response(request_id, decision)
            .await
    }
}

fn normalized_replay_capacity(replay_capacity: usize) -> NonZeroUsize {
    NonZeroUsize::new(replay_capacity)
        .or_else(|| NonZeroUsize::new(ChatRuntimeConfig::default().replay_capacity))
        .expect("default chat replay capacity is non-zero")
}

#[cfg(feature = "agent")]
fn spawn_subagent_runtime_event_bridge(
    controller: Arc<crate::agent::subagents::SubagentRoutingController>,
    chat_projector: Arc<StdMutex<ChatProjector>>,
    runtime_event_publish_tx: mpsc::UnboundedSender<RuntimeEventPublishRequest>,
    runtime_event_send_lock: Arc<StdMutex<()>>,
) {
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::time::{Duration, Instant};

    use crate::agent::runtime::chat::{
        AgentRuntimeError, AgentRuntimeEventPayload, MessageStatus, SubagentMessageSnapshot,
        SubagentRuntimeSnapshot, SubagentToolCallSnapshot, ThreadId, ToolStatus,
    };
    use crate::agent::subagents::{SubagentEvent, SubagentId, SubagentRoutingMetadata};
    use crate::agent_loop::AgentEvent;
    use crate::human_interaction::HumanInteractionPayload;
    use crate::tools::AskUserPrompt;

    const PENDING_EVENT_CAP: usize = 64;
    const PENDING_EVENT_TTL: Duration = Duration::from_secs(300);

    struct PendingSubagentEvents {
        first_seen: Instant,
        events: VecDeque<SubagentEvent>,
    }

    let mut events = controller.subscribe();
    let controller = Arc::downgrade(&controller);
    tokio::spawn(async move {
        let mut sequences: HashMap<SubagentId, u64> = HashMap::new();
        let mut started: HashSet<SubagentId> = HashSet::new();
        let mut pending: HashMap<SubagentId, PendingSubagentEvents> = HashMap::new();
        let projection_error = Arc::new(StdMutex::new(None));

        'events: loop {
            log_projection_error(&projection_error, None, None, None);
            purge_stale_pending(&mut pending);
            let event = match events.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            let subagent_id = subagent_event_id(&event);
            if !matches!(event, SubagentEvent::Spawned { .. }) && !started.contains(&subagent_id) {
                if matches!(
                    event,
                    SubagentEvent::Failed { .. } | SubagentEvent::Aborted { .. }
                ) {
                    pending.remove(&subagent_id);
                    continue;
                }
                let entry = pending
                    .entry(subagent_id)
                    .or_insert_with(|| PendingSubagentEvents {
                        first_seen: Instant::now(),
                        events: VecDeque::new(),
                    });
                if entry.events.len() == PENDING_EVENT_CAP {
                    entry.events.pop_front();
                }
                entry.events.push_back(event);
                continue;
            }

            let mut events_to_project = vec![event];
            if matches!(events_to_project[0], SubagentEvent::Spawned { .. }) {
                started.insert(subagent_id);
                if let Some(buffered) = pending.remove(&subagent_id) {
                    events_to_project.extend(buffered.events);
                }
            }

            for event in events_to_project {
                let Some(controller) = controller.upgrade() else {
                    break 'events;
                };
                let subagent_id = subagent_event_id(&event);
                let sequence = sequences
                    .entry(subagent_id)
                    .and_modify(|value| *value += 1)
                    .or_insert(1);
                let metadata = controller.metadata(subagent_id).await;
                let (thread_id, active_turn_id) = match chat_projector.lock() {
                    Ok(projector) => {
                        let thread_id = projector.default_thread_id();
                        let active_turn_id = projector
                            .read_thread(thread_id)
                            .ok()
                            .and_then(|thread| thread.active_turn_id);
                        (thread_id, active_turn_id)
                    }
                    Err(_) => continue,
                };
                let Some(payload) =
                    subagent_runtime_payload(event, metadata, active_turn_id, *sequence)
                else {
                    continue;
                };
                let projected = match chat_projector.lock() {
                    Ok(mut projector) => {
                        if let Some(turn_id) = active_turn_id {
                            projector
                                .record_subagent_event(turn_id, payload.clone())
                                .or_else(|_| {
                                    projector.record_subagent_event_for_thread(thread_id, payload)
                                })
                        } else {
                            projector.record_subagent_event_for_thread(thread_id, payload)
                        }
                    }
                    Err(_) => continue,
                };
                let projected = match projected {
                    Ok(projected) => projected,
                    Err(err) => {
                        log_chat_projection_error(subagent_id, *sequence, Some(thread_id), &err);
                        continue;
                    }
                };
                if let Err(err) = AgentRuntime::queue_runtime_event_to(
                    &runtime_event_publish_tx,
                    &runtime_event_send_lock,
                    projected,
                    projection_error.clone(),
                ) {
                    log_chat_projection_error(subagent_id, *sequence, Some(thread_id), &err);
                    continue;
                }
                log_projection_error(
                    &projection_error,
                    Some(subagent_id),
                    Some(*sequence),
                    Some(thread_id),
                );
            }
        }
    });

    fn purge_stale_pending(pending: &mut HashMap<SubagentId, PendingSubagentEvents>) {
        let now = Instant::now();
        pending.retain(|_, entry| now.duration_since(entry.first_seen) <= PENDING_EVENT_TTL);
    }

    fn log_projection_error(
        slot: &Arc<StdMutex<Option<AgentRuntimeError>>>,
        subagent_id: Option<SubagentId>,
        sequence: Option<u64>,
        thread_id: Option<ThreadId>,
    ) {
        let Some(err) = slot.lock().ok().and_then(|mut error| error.take()) else {
            return;
        };
        log_chat_projection_error_context(subagent_id, sequence, thread_id, &err);
    }

    fn log_chat_projection_error(
        subagent_id: SubagentId,
        sequence: u64,
        thread_id: Option<ThreadId>,
        err: &AgentRuntimeError,
    ) {
        log_chat_projection_error_context(Some(subagent_id), Some(sequence), thread_id, err);
    }

    fn log_chat_projection_error_context(
        subagent_id: Option<SubagentId>,
        sequence: Option<u64>,
        thread_id: Option<ThreadId>,
        err: &AgentRuntimeError,
    ) {
        eprintln!(
            "roci subagent projection failed subagent_id={} sequence={} thread_id={} error={err}",
            subagent_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            sequence
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            thread_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
        );
    }

    fn subagent_event_id(event: &SubagentEvent) -> SubagentId {
        match event {
            SubagentEvent::Spawned { subagent_id, .. }
            | SubagentEvent::StatusChanged { subagent_id, .. }
            | SubagentEvent::AgentEvent { subagent_id, .. }
            | SubagentEvent::Completed { subagent_id, .. }
            | SubagentEvent::Failed { subagent_id, .. }
            | SubagentEvent::Aborted { subagent_id } => *subagent_id,
        }
    }

    fn subagent_runtime_payload(
        event: SubagentEvent,
        metadata: Option<SubagentRoutingMetadata>,
        parent_turn_id: Option<TurnId>,
        sequence: u64,
    ) -> Option<AgentRuntimeEventPayload> {
        match event {
            SubagentEvent::Spawned {
                subagent_id,
                label,
                profile,
                model,
            } => Some(AgentRuntimeEventPayload::SubagentStarted {
                subagent: subagent_snapshot(
                    subagent_id,
                    metadata,
                    profile,
                    label,
                    model,
                    crate::agent::subagents::SubagentStatus::Running,
                    parent_turn_id,
                    sequence,
                ),
            }),
            SubagentEvent::StatusChanged {
                subagent_id,
                status,
            } => Some(AgentRuntimeEventPayload::SubagentProgress {
                subagent: subagent_snapshot_from_metadata(
                    subagent_id,
                    metadata,
                    status,
                    parent_turn_id,
                    sequence,
                ),
                message: Some(format!("status: {status:?}")),
            }),
            SubagentEvent::AgentEvent {
                subagent_id,
                label,
                event,
            } => child_agent_payload(
                subagent_id,
                label,
                *event,
                metadata,
                parent_turn_id,
                sequence,
            ),
            SubagentEvent::Completed {
                subagent_id,
                result,
            } => {
                let profile_id = metadata
                    .as_ref()
                    .map(|metadata| metadata.profile_id.clone())
                    .unwrap_or_else(|| "unknown".to_string());
                let child_thread_id = metadata
                    .as_ref()
                    .and_then(|metadata| metadata.child_thread_id);
                Some(AgentRuntimeEventPayload::SubagentCompleted {
                    subagent: subagent_snapshot_from_metadata(
                        subagent_id,
                        metadata,
                        result.status,
                        parent_turn_id,
                        sequence,
                    ),
                    result: crate::agent::subagents::routing::compact_result_for_runtime(
                        &profile_id,
                        &result,
                        child_thread_id,
                    ),
                })
            }
            SubagentEvent::Failed { subagent_id, error } => {
                Some(AgentRuntimeEventPayload::SubagentFailed {
                    subagent: subagent_snapshot_from_metadata(
                        subagent_id,
                        metadata,
                        crate::agent::subagents::SubagentStatus::Failed,
                        parent_turn_id,
                        sequence,
                    ),
                    error,
                })
            }
            SubagentEvent::Aborted { subagent_id } => {
                Some(AgentRuntimeEventPayload::SubagentCancelled {
                    subagent: subagent_snapshot_from_metadata(
                        subagent_id,
                        metadata,
                        crate::agent::subagents::SubagentStatus::Aborted,
                        parent_turn_id,
                        sequence,
                    ),
                })
            }
        }
    }

    fn child_agent_payload(
        subagent_id: SubagentId,
        label: Option<String>,
        event: AgentEvent,
        metadata: Option<SubagentRoutingMetadata>,
        parent_turn_id: Option<TurnId>,
        sequence: u64,
    ) -> Option<AgentRuntimeEventPayload> {
        let fallback_profile = metadata
            .as_ref()
            .map(|metadata| metadata.profile_id.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let fallback_model = metadata
            .as_ref()
            .and_then(|metadata| metadata.model.clone());
        let status = crate::agent::subagents::SubagentStatus::Running;
        let subagent = subagent_snapshot(
            subagent_id,
            metadata,
            fallback_profile,
            label,
            fallback_model,
            status,
            parent_turn_id,
            sequence,
        );
        match event {
            AgentEvent::MessageUpdate { message, .. } => {
                Some(AgentRuntimeEventPayload::SubagentProgress {
                    subagent,
                    message: Some(message.text()),
                })
            }
            AgentEvent::MessageEnd { message } => Some(AgentRuntimeEventPayload::SubagentMessage {
                subagent,
                message: SubagentMessageSnapshot {
                    role: message.role,
                    text: message.text(),
                    status: MessageStatus::Completed,
                },
            }),
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => Some(AgentRuntimeEventPayload::SubagentToolCallStarted {
                subagent,
                tool: SubagentToolCallSnapshot {
                    tool_call_id,
                    tool_name,
                    args,
                    result: None,
                    status: ToolStatus::Running,
                },
            }),
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error: _,
            } => Some(AgentRuntimeEventPayload::SubagentToolCallCompleted {
                subagent,
                tool: SubagentToolCallSnapshot {
                    tool_call_id,
                    tool_name,
                    args: serde_json::Value::Null,
                    result: Some(result),
                    status: ToolStatus::Completed,
                },
            }),
            AgentEvent::HumanInteractionRequested { request } => {
                let (question, context) = match &request.payload {
                    HumanInteractionPayload::AskUser(payload) => ask_user_question(&payload.prompt),
                    _ => ("Child sub-agent requested input".to_string(), None),
                };
                Some(AgentRuntimeEventPayload::SubagentNeedsInput {
                    subagent,
                    question,
                    context,
                })
            }
            _ => None,
        }
    }

    fn subagent_snapshot_from_metadata(
        subagent_id: SubagentId,
        metadata: Option<SubagentRoutingMetadata>,
        status: crate::agent::subagents::SubagentStatus,
        parent_turn_id: Option<TurnId>,
        sequence: u64,
    ) -> SubagentRuntimeSnapshot {
        let profile = metadata
            .as_ref()
            .map(|metadata| metadata.profile_id.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let label = metadata
            .as_ref()
            .and_then(|metadata| metadata.label.clone());
        let model = metadata
            .as_ref()
            .and_then(|metadata| metadata.model.clone());
        subagent_snapshot(
            subagent_id,
            metadata,
            profile,
            label,
            model,
            status,
            parent_turn_id,
            sequence,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn subagent_snapshot(
        subagent_id: SubagentId,
        metadata: Option<SubagentRoutingMetadata>,
        profile_id: String,
        label: Option<String>,
        model: Option<LanguageModel>,
        status: crate::agent::subagents::SubagentStatus,
        parent_turn_id: Option<TurnId>,
        sequence: u64,
    ) -> SubagentRuntimeSnapshot {
        SubagentRuntimeSnapshot {
            subagent_id,
            profile_id,
            label,
            status,
            model,
            parent_turn_id,
            parent_tool_call_id: metadata
                .as_ref()
                .and_then(|metadata| metadata.parent_tool_call_id.clone()),
            child_thread_id: metadata
                .as_ref()
                .and_then(|metadata| metadata.child_thread_id),
            source_subagent_id: metadata
                .as_ref()
                .and_then(|metadata| metadata.source_subagent_id),
            target_subagent_id: metadata
                .as_ref()
                .and_then(|metadata| metadata.target_subagent_id),
            sequence,
        }
    }

    fn ask_user_question(prompt: &AskUserPrompt) -> (String, Option<String>) {
        match prompt {
            AskUserPrompt::Question { question, .. }
            | AskUserPrompt::Confirm { question, .. }
            | AskUserPrompt::Choice { question, .. }
            | AskUserPrompt::MultiChoice { question, .. } => (question.clone(), None),
            AskUserPrompt::Form { title, .. } => (
                title
                    .clone()
                    .unwrap_or_else(|| "Child sub-agent requested input".to_string()),
                Some(prompt.id().to_string()),
            ),
        }
    }
}

#[cfg(test)]
#[path = "runtime_tests/mod.rs"]
mod tests;
