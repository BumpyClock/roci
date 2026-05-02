//! High-level Agent runtime wrapping the agent loop.
//!
//! Provides the pi-mono aligned API surface:
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
#[cfg(test)]
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider::ProviderRegistry;
#[cfg(test)]
use crate::provider::ProviderRequest;
#[cfg(test)]
use crate::resource::{BranchSummarySettings, CompactionSettings};
use crate::tools::dynamic::DynamicToolProvider;
use crate::tools::tool::Tool;
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
/// let agent = AgentRuntime::new(registry, roci_config, config);
/// let result = agent.prompt("Hello").await?;
/// let result = agent.continue_run("Tell me more").await?;
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
    model: Arc<Mutex<LanguageModel>>,
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
    pub event: AgentRuntimeEvent,
    pub ack_tx: Option<oneshot::Sender<Result<RuntimeCursor, AgentRuntimeError>>>,
    pub error_slot: Option<Arc<StdMutex<Option<AgentRuntimeError>>>>,
}

impl AgentRuntime {
    /// Create a new agent runtime with the given configuration.
    pub fn new(
        registry: Arc<ProviderRegistry>,
        roci_config: RociConfig,
        config: AgentConfig,
    ) -> Self {
        let runner = LoopRunner::with_registry(roci_config.clone(), registry.clone());
        let model = Arc::new(Mutex::new(config.model.clone()));
        let generation_settings = Arc::new(Mutex::new(config.settings.clone()));
        let approval_policy = Arc::new(Mutex::new(config.approval_policy));
        let system_prompt = Arc::new(Mutex::new(config.system_prompt.clone()));
        let tools = Arc::new(Mutex::new(config.tools.clone()));
        let dynamic_tool_providers = Arc::new(Mutex::new(config.dynamic_tool_providers.clone()));
        let replay_capacity = normalized_replay_capacity(config.chat.replay_capacity);
        let runtime_event_store = config.chat.event_store.clone().unwrap_or_else(|| {
            Arc::new(InMemoryAgentRuntimeEventStore::with_replay_capacity(
                replay_capacity,
            ))
        });
        let (runtime_event_tx, _) = broadcast::channel(replay_capacity.get());
        let (runtime_event_publish_tx, runtime_event_publish_rx) =
            mpsc::unbounded_channel::<RuntimeEventPublishRequest>();
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
        Self {
            config,
            runner,
            roci_config,
            registry,
            state: Arc::new(Mutex::new(AgentState::Idle)),
            state_tx,
            state_rx,
            model,
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
            runtime_event_send_lock: Arc::new(StdMutex::new(())),
            runtime_event_publish_tx,
            runtime_event_publish_rx: Arc::new(Mutex::new(Some(runtime_event_publish_rx))),
            session_usage: Arc::new(Mutex::new(Usage::default())),
            #[cfg(feature = "agent")]
            human_interaction_coordinator,
            #[cfg(feature = "agent")]
            tool_permission_session_approvals,
        }
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

#[cfg(test)]
#[path = "runtime_tests/mod.rs"]
mod tests;
