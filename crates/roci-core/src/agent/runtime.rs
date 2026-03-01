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
mod run_loop;
mod summary;
mod types;

pub use self::config::AgentConfig;
#[cfg(test)]
use self::types::drain_queue;
pub use self::types::{
    AgentSnapshot, AgentState, GetApiKeyFn, QueueDrainMode, SessionBeforeCompactHook,
    SessionBeforeCompactPayload, SessionBeforeTreeHook, SessionBeforeTreePayload,
    SessionCompactionOverride, SessionSummaryHookOutcome, SummaryPreparationData,
};

#[cfg(test)]
use crate::agent_loop::runner::BeforeAgentStartHookResult;

#[cfg(test)]
use crate::agent::message::AgentMessageExt;
#[cfg(test)]
use crate::agent_loop::compaction::{
    estimate_message_tokens, serialize_pi_mono_summary, PiMonoSummary,
};
use crate::agent_loop::LoopRunner;
#[cfg(test)]
use crate::agent_loop::RunStatus;
use crate::config::RociConfig;
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
        }
    }

    /// Current agent state.
    pub async fn state(&self) -> AgentState {
        *self.state.lock().await
    }

    /// Subscribe to state changes via a [`watch::Receiver`].
    ///
    /// Callers can `.changed().await` on the returned receiver to be notified
    /// whenever the agent transitions between states.
    pub fn watch_state(&self) -> watch::Receiver<AgentState> {
        self.state_rx.clone()
    }

    /// Get a snapshot of the current message history.
    pub async fn messages(&self) -> Vec<ModelMessage> {
        self.messages.lock().await.clone()
    }

    /// Get a point-in-time snapshot of agent observable state.
    pub async fn snapshot(&self) -> AgentSnapshot {
        AgentSnapshot {
            state: *self.state.lock().await,
            turn_index: *self.turn_index.lock().await,
            message_count: self.messages.lock().await.len(),
            is_streaming: *self.is_streaming.lock().await,
            last_error: self.last_error.lock().await.clone(),
        }
    }

    /// Subscribe to snapshot changes via a [`watch::Receiver`].
    ///
    /// Callers can `.changed().await` on the returned receiver to be notified
    /// whenever any observable field in the snapshot changes.
    pub fn watch_snapshot(&self) -> watch::Receiver<AgentSnapshot> {
        self.snapshot_rx.clone()
    }

    /// Replace the configured system prompt.
    ///
    /// Runtime mutators are allowed only when idle. This method fails fast if a
    /// run is active or the runtime state lock is contended.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn set_system_prompt(&self, prompt: impl Into<String>) -> Result<(), RociError> {
        let _state_guard = self.lock_state_for_idle_mutation()?;
        let mut system_prompt = self.system_prompt.try_lock().map_err(|_| {
            RociError::InvalidState("Agent is busy (system prompt lock contended)".into())
        })?;
        *system_prompt = Some(prompt.into());
        Ok(())
    }

    /// Replace the configured model used for subsequent runs.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn set_model(&self, model: LanguageModel) -> Result<(), RociError> {
        let _state_guard = self.lock_state_for_idle_mutation()?;
        let mut runtime_model = self
            .model
            .try_lock()
            .map_err(|_| RociError::InvalidState("Agent is busy (model lock contended)".into()))?;
        *runtime_model = model;
        Ok(())
    }

    /// Clear the configured system prompt.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn clear_system_prompt(&self) -> Result<(), RociError> {
        let _state_guard = self.lock_state_for_idle_mutation()?;
        let mut system_prompt = self.system_prompt.try_lock().map_err(|_| {
            RociError::InvalidState("Agent is busy (system prompt lock contended)".into())
        })?;
        *system_prompt = None;
        Ok(())
    }

    /// Replace the full conversation message history.
    ///
    /// This is an atomic replace operation and does not enqueue a run.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn replace_messages(&self, messages: Vec<ModelMessage>) -> Result<(), RociError> {
        let state_guard = self.lock_state_for_idle_mutation()?;
        let mut existing_messages = self.messages.try_lock().map_err(|_| {
            RociError::InvalidState("Agent is busy (messages lock contended)".into())
        })?;
        *existing_messages = messages;
        drop(existing_messages);
        drop(state_guard);
        self.broadcast_snapshot().await;
        Ok(())
    }

    /// Replace the tool registry used for subsequent runs.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn set_tools(&self, tools: Vec<Arc<dyn Tool>>) -> Result<(), RociError> {
        let _state_guard = self.lock_state_for_idle_mutation()?;
        let mut runtime_tools = self
            .tools
            .try_lock()
            .map_err(|_| RociError::InvalidState("Agent is busy (tools lock contended)".into()))?;
        *runtime_tools = tools;
        Ok(())
    }

    /// Replace the dynamic tool providers used for subsequent runs.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn set_dynamic_tool_providers(
        &self,
        providers: Vec<Arc<dyn DynamicToolProvider>>,
    ) -> Result<(), RociError> {
        let _state_guard = self.lock_state_for_idle_mutation()?;
        let mut runtime_providers = self.dynamic_tool_providers.try_lock().map_err(|_| {
            RociError::InvalidState("Agent is busy (dynamic tool lock contended)".into())
        })?;
        *runtime_providers = providers;
        Ok(())
    }

    /// Clear all dynamic tool providers.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn clear_dynamic_tool_providers(&self) -> Result<(), RociError> {
        self.set_dynamic_tool_providers(Vec::new()).await
    }

    /// Clear all queued steering messages.
    pub async fn clear_steering_queue(&self) {
        self.steering_queue.lock().await.clear();
    }

    /// Clear all queued follow-up messages.
    pub async fn clear_follow_up_queue(&self) {
        self.follow_up_queue.lock().await.clear();
    }

    /// Clear both steering and follow-up queues.
    pub async fn clear_all_queues(&self) {
        self.steering_queue.lock().await.clear();
        self.follow_up_queue.lock().await.clear();
    }

    /// Returns true when either steering or follow-up queue has at least one message.
    pub async fn has_queued_messages(&self) -> bool {
        !self.steering_queue.lock().await.is_empty()
            || !self.follow_up_queue.lock().await.is_empty()
    }

    // -- Internal helpers --

    /// Broadcast the current snapshot to all watchers.
    async fn broadcast_snapshot(&self) {
        let snapshot = self.snapshot().await;
        let _ = self.snapshot_tx.send(snapshot);
    }

    /// Atomically transition from Idle → Running.
    ///
    /// Uses a try_lock + immediate check to fail fast without holding the
    /// lock across an await point.
    fn transition_to_running(&self) -> Result<(), RociError> {
        // Use `try_lock` to avoid holding the mutex across the caller's await.
        // If the lock is contended, another task is already mutating state and
        // we can safely report the agent is busy.
        let mut state = self
            .state
            .try_lock()
            .map_err(|_| RociError::InvalidState("Agent is busy (state lock contended)".into()))?;
        if *state != AgentState::Idle {
            return Err(RociError::InvalidState(
                "Agent is not idle; call abort() or wait_for_idle() first".into(),
            ));
        }
        *state = AgentState::Running;
        let _ = self.state_tx.send(AgentState::Running);
        Ok(())
    }

    fn lock_state_for_idle_mutation(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, AgentState>, RociError> {
        let state = self
            .state
            .try_lock()
            .map_err(|_| RociError::InvalidState("Agent is busy (state lock contended)".into()))?;
        if *state != AgentState::Idle {
            return Err(RociError::InvalidState(
                "Agent is not idle; runtime mutation requires idle state".into(),
            ));
        }
        Ok(state)
    }

    async fn restore_idle_after_preflight_error(&self) {
        let mut state = self.state.lock().await;
        *state = AgentState::Idle;
        let _ = self.state_tx.send(AgentState::Idle);
        drop(state);
        self.broadcast_snapshot().await;
        self.idle_notify.notify_waiters();
    }
}

#[cfg(test)]
#[path = "runtime_tests/mod.rs"]
mod tests;
