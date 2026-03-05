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
use std::sync::Arc;
use tokio::sync::{oneshot, watch, Mutex, Notify};

mod config;
mod events;
mod lifecycle;
mod mutations;
mod run_loop;
mod state;
mod summary;
mod types;
#[cfg(feature = "agent")]
mod user_input;

#[cfg(feature = "agent")]
pub use self::user_input::UserInputCoordinator;

pub use self::config::AgentConfig;
#[cfg(test)]
use self::types::drain_queue;
pub use self::types::{
    AgentSnapshot, AgentState, GetApiKeyFn, QueueDrainMode, SessionBeforeCompactHook,
    SessionBeforeCompactPayload, SessionBeforeTreeHook, SessionBeforeTreePayload,
    SessionCompactionOverride, SessionSummaryHookOutcome, SummaryPreparationData,
};

#[cfg(test)]
use crate::agent::message::AgentMessageExt;
#[cfg(test)]
use crate::agent_loop::compaction::{
    estimate_message_tokens, serialize_pi_mono_summary, PiMonoSummary,
};
#[cfg(test)]
use crate::agent_loop::runner::BeforeAgentStartHookResult;
use crate::agent_loop::LoopRunner;
#[cfg(test)]
use crate::agent_loop::RunStatus;
use crate::config::RociConfig;
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
use crate::types::GenerationSettings;
use crate::types::ModelMessage;
#[cfg(test)]
use crate::types::Role;

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
pub struct AgentRuntime {
    config: AgentConfig,
    runner: LoopRunner,
    roci_config: RociConfig,
    registry: Arc<ProviderRegistry>,
    state: Arc<Mutex<AgentState>>,
    state_tx: watch::Sender<AgentState>,
    state_rx: watch::Receiver<AgentState>,
    model: Arc<Mutex<LanguageModel>>,
    system_prompt: Arc<Mutex<Option<String>>>,
    tools: Arc<Mutex<Vec<Arc<dyn Tool>>>>,
    dynamic_tool_providers: Arc<Mutex<Vec<Arc<dyn DynamicToolProvider>>>>,
    messages: Arc<Mutex<Vec<ModelMessage>>>,
    steering_queue: Arc<Mutex<Vec<ModelMessage>>>,
    follow_up_queue: Arc<Mutex<Vec<ModelMessage>>>,
    active_abort_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    idle_notify: Arc<Notify>,
    turn_index: Arc<Mutex<usize>>,
    is_streaming: Arc<Mutex<bool>>,
    last_error: Arc<Mutex<Option<String>>>,
    snapshot_tx: watch::Sender<AgentSnapshot>,
    snapshot_rx: watch::Receiver<AgentSnapshot>,
    #[cfg(feature = "agent")]
    user_input_coordinator: Arc<UserInputCoordinator>,
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
        let system_prompt = Arc::new(Mutex::new(config.system_prompt.clone()));
        let tools = Arc::new(Mutex::new(config.tools.clone()));
        let dynamic_tool_providers = Arc::new(Mutex::new(config.dynamic_tool_providers.clone()));
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
        let user_input_coordinator = config
            .user_input_coordinator
            .clone()
            .unwrap_or_else(|| Arc::new(UserInputCoordinator::new()));
        Self {
            config,
            runner,
            roci_config,
            registry,
            state: Arc::new(Mutex::new(AgentState::Idle)),
            state_tx,
            state_rx,
            model,
            system_prompt,
            tools,
            dynamic_tool_providers,
            messages: Arc::new(Mutex::new(Vec::new())),
            steering_queue: Arc::new(Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            active_abort_tx: Arc::new(Mutex::new(None)),
            idle_notify: Arc::new(Notify::new()),
            turn_index: Arc::new(Mutex::new(0)),
            is_streaming: Arc::new(Mutex::new(false)),
            last_error: Arc::new(Mutex::new(None)),
            snapshot_tx,
            snapshot_rx,
            #[cfg(feature = "agent")]
            user_input_coordinator,
        }
    }

    /// Submit a user input response.
    ///
    /// This is called by the CLI/host when a user responds to a `UserInputRequested` event.
    /// The response will be routed to the waiting tool execution.
    #[cfg(feature = "agent")]
    pub async fn submit_user_input(
        &self,
        response: crate::tools::UserInputResponse,
    ) -> Result<(), crate::tools::UnknownUserInputRequest> {
        self.user_input_coordinator.submit_response(response).await
    }
}

#[cfg(test)]
#[path = "runtime_tests/mod.rs"]
mod tests;
