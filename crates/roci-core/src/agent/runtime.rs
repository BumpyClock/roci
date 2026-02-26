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

use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{oneshot, watch, Mutex, Notify};

use crate::agent::message::{AgentMessage, AgentMessageExt};
use crate::agent_loop::runner::{
    AgentEventSink, AutoCompactionConfig, CompactionHandler, ConvertToLlmFn, FollowUpMessagesFn,
    RunHooks, SteeringMessagesFn, TransformContextFn,
};
use crate::agent_loop::{
    compaction::{
        extract_file_operations, prepare_compaction, serialize_messages_for_summary,
        serialize_pi_mono_summary, PiMonoSummary,
    },
    AgentEvent, LoopRunner, RunHandle, RunRequest, RunResult, RunStatus, Runner,
};
use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider::{ProviderRegistry, ProviderRequest};
use crate::resource::CompactionSettings;
use crate::tools::dynamic::{DynamicToolAdapter, DynamicToolProvider};
use crate::tools::tool::Tool;
use crate::types::{GenerationSettings, ModelMessage, Role};

/// Agent runtime state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    /// No run in progress; ready to accept prompts.
    Idle,
    /// A run is actively executing.
    Running,
    /// An abort has been requested; waiting for the run to wind down.
    Aborting,
}

/// Queue drain behavior for steering/follow-up messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueDrainMode {
    /// Drain all queued messages at once.
    All,
    /// Drain at most one message per turn/phase.
    OneAtATime,
}

fn drain_queue(queue: &mut Vec<ModelMessage>, mode: QueueDrainMode) -> Vec<ModelMessage> {
    match mode {
        QueueDrainMode::All => std::mem::take(queue),
        QueueDrainMode::OneAtATime => {
            if queue.is_empty() {
                Vec::new()
            } else {
                vec![queue.remove(0)]
            }
        }
    }
}

/// Point-in-time snapshot of agent observable state.
///
/// Captures all externally observable dimensions of an [`AgentRuntime`] at a
/// single instant. Subscribe to changes via [`AgentRuntime::watch_snapshot`].
///
/// # Example
///
/// ```ignore
/// let snap = agent.snapshot().await;
/// println!("turn {}, {} messages", snap.turn_index, snap.message_count);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct AgentSnapshot {
    pub state: AgentState,
    pub turn_index: usize,
    pub message_count: usize,
    pub is_streaming: bool,
    pub last_error: Option<String>,
}

/// Async callback that resolves an API key at request time.
///
/// Enables token rotation and dynamic key resolution without rebuilding the
/// agent. The callback is invoked once per run, before the [`RunRequest`] is
/// dispatched to the inner loop.
///
/// # Example
///
/// ```ignore
/// let get_key: GetApiKeyFn = Arc::new(|| {
///     Box::pin(async { Ok("sk-live-rotated-key".to_string()) })
/// });
/// ```
pub type GetApiKeyFn =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<String, RociError>> + Send>> + Send + Sync>;

/// Configuration for creating an [`AgentRuntime`].
///
/// # API key resolution
///
/// By default, the agent resolves API keys automatically through the
/// [`RociConfig`] passed to [`AgentRuntime::new`]. `RociConfig` checks
/// (in order): environment variables → `credentials.json` → OAuth token
/// store (from `roci auth login`).
///
/// Set [`get_api_key`](Self::get_api_key) only when you need per-request
/// dynamic keys (e.g., token rotation or multi-tenant key injection).
pub struct AgentConfig {
    /// The language model to use for generation.
    pub model: LanguageModel,
    /// Optional system prompt prepended to the first turn.
    pub system_prompt: Option<String>,
    /// Tools available for tool-use loops.
    pub tools: Vec<Arc<dyn Tool>>,
    /// Dynamic tool providers queried at run start.
    pub dynamic_tool_providers: Vec<Arc<dyn DynamicToolProvider>>,
    /// Generation settings (temperature, max_tokens, etc.).
    pub settings: GenerationSettings,
    /// Optional hook to transform the message context before each LLM call.
    pub transform_context: Option<TransformContextFn>,
    /// Optional hook to convert/filter agent-level messages before provider requests.
    pub convert_to_llm: Option<ConvertToLlmFn>,
    /// Optional sink for high-level [`AgentEvent`](crate::agent_loop::AgentEvent) emission.
    pub event_sink: Option<AgentEventSink>,
    /// Optional session ID for provider-side prompt caching.
    pub session_id: Option<String>,
    /// Drain mode for steering queue retrieval.
    pub steering_mode: QueueDrainMode,
    /// Drain mode for follow-up queue retrieval.
    pub follow_up_mode: QueueDrainMode,
    /// Optional provider transport preference.
    pub transport: Option<String>,
    /// Optional cap for server-requested retry delays in milliseconds.
    /// `Some(0)` disables the cap.
    pub max_retry_delay_ms: Option<u64>,
    /// Optional async callback to resolve an API key per run.
    ///
    /// When set, called at the start of each run. The resolved key is
    /// inserted into [`RunRequest::metadata`] under `"api_key"`.
    ///
    /// When `None` (the default), the agent resolves keys automatically
    /// through [`RociConfig`] which checks: environment variables →
    /// `credentials.json` → OAuth token store (from `roci auth login`).
    /// No explicit key configuration is needed if any of those sources
    /// has a valid credential for the provider.
    pub get_api_key: Option<GetApiKeyFn>,
    /// Compaction policy and summarization model selection.
    pub compaction: CompactionSettings,
}

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

    /// Start a new conversation with a user prompt.
    ///
    /// If the message history is empty and a system prompt is configured,
    /// the system prompt is prepended automatically.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the agent is not idle.
    pub async fn prompt(&self, text: impl Into<String>) -> Result<RunResult, RociError> {
        self.transition_to_running()?;

        let text = text.into();
        let system_prompt = self.system_prompt.lock().await.clone();
        let mut msgs = self.messages.lock().await;
        if let Some(ref sys) = system_prompt {
            if msgs.is_empty() {
                msgs.push(ModelMessage::system(sys.clone()));
            }
        }
        msgs.push(ModelMessage::user(text));
        let snapshot = msgs.clone();
        drop(msgs);

        self.run_loop(snapshot).await
    }

    /// Continue the conversation with additional user input.
    ///
    /// Unlike [`prompt`](Self::prompt), this never prepends the system prompt
    /// (it was already added on the first turn).
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the agent is not idle.
    pub async fn continue_run(&self, text: impl Into<String>) -> Result<RunResult, RociError> {
        self.transition_to_running()?;

        let text = text.into();
        let mut msgs = self.messages.lock().await;
        msgs.push(ModelMessage::user(text));
        let snapshot = msgs.clone();
        drop(msgs);

        self.run_loop(snapshot).await
    }

    /// Continue the conversation without appending a new user message.
    ///
    /// This mirrors pi-mono's `continue()` behavior and is useful for retrying
    /// from existing context or draining queued steering/follow-up messages.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if:
    /// - the agent is not idle,
    /// - there is no message history to continue from,
    /// - the last message is assistant and there are no queued steering/follow-ups.
    pub async fn continue_without_input(&self) -> Result<RunResult, RociError> {
        self.transition_to_running()?;

        let snapshot = self.messages.lock().await.clone();
        if snapshot.is_empty() {
            self.restore_idle_after_preflight_error().await;
            return Err(RociError::InvalidState(
                "No messages to continue from".into(),
            ));
        }

        if matches!(snapshot.last().map(|m| m.role), Some(Role::Assistant)) {
            let has_steering = !self.steering_queue.lock().await.is_empty();
            let has_follow_ups = !self.follow_up_queue.lock().await.is_empty();
            if !has_steering && !has_follow_ups {
                self.restore_idle_after_preflight_error().await;
                return Err(RociError::InvalidState(
                    "Cannot continue from message role: assistant".into(),
                ));
            }
        }

        self.run_loop(snapshot).await
    }

    /// Queue a steering message to interrupt the current tool execution.
    ///
    /// The message is injected between tool batches on the next iteration.
    /// Does nothing if the agent is idle (the message is still queued and
    /// will be picked up on the next run).
    pub async fn steer(&self, text: impl Into<String>) {
        self.steering_queue
            .lock()
            .await
            .push(ModelMessage::user(text));
    }

    /// Queue a follow-up message to continue after natural completion.
    ///
    /// Follow-up messages are checked when the inner loop ends (no more
    /// tool calls). If present, they extend the conversation.
    pub async fn follow_up(&self, text: impl Into<String>) {
        self.follow_up_queue
            .lock()
            .await
            .push(ModelMessage::user(text));
    }

    /// Abort the current run.
    ///
    /// Returns `true` if an abort signal was successfully sent, `false` if
    /// the agent was not running or the handle was already consumed.
    pub async fn abort(&self) -> bool {
        let mut state = self.state.lock().await;
        if *state != AgentState::Running {
            return false;
        }
        *state = AgentState::Aborting;
        let _ = self.state_tx.send(AgentState::Aborting);
        drop(state);
        self.broadcast_snapshot().await;

        let mut abort_tx = self.active_abort_tx.lock().await;
        if let Some(tx) = abort_tx.take() {
            tx.send(()).is_ok()
        } else {
            false
        }
    }

    /// Reset the agent: abort any in-flight run, then clear messages and queues.
    pub async fn reset(&self) {
        self.abort().await;
        self.wait_for_idle().await;

        self.messages.lock().await.clear();
        self.steering_queue.lock().await.clear();
        self.follow_up_queue.lock().await.clear();
        *self.turn_index.lock().await = 0;
        *self.is_streaming.lock().await = false;
        *self.last_error.lock().await = None;

        let mut state = self.state.lock().await;
        *state = AgentState::Idle;
        let _ = self.state_tx.send(AgentState::Idle);
        drop(state);
        self.broadcast_snapshot().await;
    }

    /// Wait until the agent is idle.
    ///
    /// Returns immediately if already idle; otherwise blocks until the
    /// current run completes, fails, or is aborted.
    pub async fn wait_for_idle(&self) {
        loop {
            if *self.state.lock().await == AgentState::Idle {
                return;
            }
            self.idle_notify.notified().await;
        }
    }

    /// Compact the current conversation history in place using the configured
    /// compaction policy and summary model.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn compact(&self) -> Result<(), RociError> {
        let state_guard = self.lock_state_for_idle_mutation()?;
        let model = self
            .model
            .try_lock()
            .map_err(|_| RociError::InvalidState("Agent is busy (model lock contended)".into()))?
            .clone();
        let messages = self
            .messages
            .try_lock()
            .map_err(|_| RociError::InvalidState("Agent is busy (messages lock contended)".into()))?
            .clone();
        drop(state_guard);

        let compacted = Self::compact_messages_with_model(
            messages,
            &model,
            &self.config.compaction,
            &self.registry,
            &self.roci_config,
        )
        .await?;

        if let Some(compacted_messages) = compacted {
            *self.messages.lock().await = compacted_messages;
            self.broadcast_snapshot().await;
        }

        Ok(())
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

    async fn resolve_tools_for_run(&self) -> Result<Vec<Arc<dyn Tool>>, RociError> {
        let static_tools = self.tools.lock().await.clone();
        let providers = self.dynamic_tool_providers.lock().await.clone();
        Self::merge_static_and_dynamic_tools(static_tools, providers).await
    }

    async fn merge_static_and_dynamic_tools(
        mut static_tools: Vec<Arc<dyn Tool>>,
        providers: Vec<Arc<dyn DynamicToolProvider>>,
    ) -> Result<Vec<Arc<dyn Tool>>, RociError> {
        for provider in providers {
            let discovered = provider.list_tools().await?;
            for tool in discovered {
                static_tools.push(Arc::new(DynamicToolAdapter::new(
                    Arc::clone(&provider),
                    tool,
                )));
            }
        }
        Ok(static_tools)
    }

    /// Build a [`RunRequest`], start the loop, wait for the result, then
    /// transition back to Idle.
    async fn run_loop(&self, initial_messages: Vec<ModelMessage>) -> Result<RunResult, RociError> {
        *self.is_streaming.lock().await = true;
        *self.last_error.lock().await = None;
        self.broadcast_snapshot().await;

        let steering_queue = self.steering_queue.clone();
        let follow_up_queue = self.follow_up_queue.clone();
        let steering_mode = self.config.steering_mode;
        let follow_up_mode = self.config.follow_up_mode;

        let steering_fn: SteeringMessagesFn = Arc::new(move || {
            let queue = steering_queue.clone();
            Box::pin(async move {
                let mut queue = queue.lock().await;
                drain_queue(&mut queue, steering_mode)
            })
        });

        let follow_up_fn: FollowUpMessagesFn = {
            let queue = follow_up_queue.clone();
            Arc::new(move || {
                let queue = queue.clone();
                Box::pin(async move {
                    let mut queue = queue.lock().await;
                    drain_queue(&mut queue, follow_up_mode)
                })
            })
        };

        let intercepting_sink = self.build_intercepting_sink();

        let model = self.model.lock().await.clone();
        let tools = match self.resolve_tools_for_run().await {
            Ok(tools) => tools,
            Err(err) => {
                self.restore_idle_after_preflight_error().await;
                return Err(err);
            }
        };

        let mut request = RunRequest::new(model, initial_messages)
            .with_tools(tools)
            .with_steering_messages(steering_fn)
            .with_follow_up_messages(follow_up_fn)
            .with_agent_event_sink(intercepting_sink);

        if self.config.compaction.enabled {
            let compaction_settings = self.config.compaction.clone();
            let registry = self.registry.clone();
            let roci_config = self.roci_config.clone();
            let run_model = request.model.clone();
            let compaction_hook: CompactionHandler = Arc::new(move |messages| {
                let compaction_settings = compaction_settings.clone();
                let registry = registry.clone();
                let roci_config = roci_config.clone();
                let run_model = run_model.clone();
                Box::pin(async move {
                    AgentRuntime::compact_messages_with_model(
                        messages,
                        &run_model,
                        &compaction_settings,
                        &registry,
                        &roci_config,
                    )
                    .await
                })
            });
            request = request.with_hooks(RunHooks {
                compaction: Some(compaction_hook),
                tool_result_persist: None,
            });
            request = request.with_auto_compaction(AutoCompactionConfig {
                reserve_tokens: self.config.compaction.reserve_tokens,
            });
        }

        request.settings = self.config.settings.clone();

        if let Some(ref transform) = self.config.transform_context {
            request = request.with_transform_context(transform.clone());
        }
        if let Some(ref convert) = self.config.convert_to_llm {
            request = request.with_convert_to_llm(convert.clone());
        }
        if let Some(ref id) = self.config.session_id {
            request = request.with_session_id(id.clone());
        }
        if let Some(ref transport) = self.config.transport {
            request = request.with_transport(transport.clone());
        }
        if let Some(max_retry_delay_ms) = self.config.max_retry_delay_ms {
            request = request.with_max_retry_delay_ms(max_retry_delay_ms);
        }

        let run_result = async {
            if let Some(ref get_key) = self.config.get_api_key {
                let key = get_key().await?;
                request.metadata.insert("api_key".to_string(), key);
            }

            let mut handle: RunHandle = self.runner.start(request).await?;
            let abort_tx = handle.take_abort_sender();
            *self.active_abort_tx.lock().await = abort_tx;

            Ok::<RunResult, RociError>(handle.wait().await)
        }
        .await;

        self.active_abort_tx.lock().await.take();
        *self.is_streaming.lock().await = false;

        match &run_result {
            Ok(result) => {
                *self.messages.lock().await = result.messages.clone();
                if result.status == RunStatus::Failed {
                    *self.last_error.lock().await = result.error.clone();
                } else {
                    *self.last_error.lock().await = None;
                }
            }
            Err(err) => {
                *self.last_error.lock().await = Some(err.to_string());
            }
        }

        let mut state = self.state.lock().await;
        *state = AgentState::Idle;
        let _ = self.state_tx.send(AgentState::Idle);
        drop(state);
        self.broadcast_snapshot().await;
        self.idle_notify.notify_waiters();

        run_result
    }

    async fn compact_messages_with_model(
        messages: Vec<ModelMessage>,
        run_model: &LanguageModel,
        compaction: &CompactionSettings,
        registry: &Arc<ProviderRegistry>,
        roci_config: &RociConfig,
    ) -> Result<Option<Vec<ModelMessage>>, RociError> {
        let system_prefix_len = messages
            .iter()
            .take_while(|message| message.role == Role::System)
            .count();
        let system_prefix = messages[..system_prefix_len].to_vec();
        let conversation_messages = messages[system_prefix_len..].to_vec();

        if conversation_messages.len() < 2 {
            return Ok(None);
        }

        let prepared = prepare_compaction(&conversation_messages, compaction.keep_recent_tokens);
        if prepared.messages_to_summarize.is_empty() {
            return Ok(None);
        }

        let summary_model = match compaction.model.as_deref() {
            Some(model) => LanguageModel::from_str(model)?,
            None => run_model.clone(),
        };
        let provider = registry.create_provider(
            summary_model.provider_name(),
            summary_model.model_id(),
            roci_config,
        )?;

        let transcript = serialize_messages_for_summary(&prepared.messages_to_summarize);
        let summary_prompt = format!(
            "Summarize the conversation transcript into concise bullets focused on user goals, constraints, progress, decisions, next steps, and critical context.\n\nTranscript:\n{transcript}"
        );
        let summary_response = provider
            .generate_text(&ProviderRequest {
                messages: vec![
                    ModelMessage::system("You create precise conversation compaction summaries"),
                    ModelMessage::user(summary_prompt),
                ],
                settings: GenerationSettings::default(),
                tools: None,
                response_format: None,
                session_id: None,
                transport: None,
            })
            .await?;

        let summary_text = summary_response.text.trim().to_string();
        if summary_text.is_empty() {
            return Err(RociError::InvalidState(
                "compaction summary model returned empty output".to_string(),
            ));
        }

        let file_ops = extract_file_operations(&prepared.messages_to_summarize);
        let summary = PiMonoSummary {
            progress: vec![summary_text],
            critical_context: if prepared.split_turn {
                vec!["A turn split was preserved to avoid cutting a user/tool exchange".to_string()]
            } else {
                Vec::new()
            },
            read_files: file_ops.read_files,
            modified_files: file_ops.modified_files,
            ..PiMonoSummary::default()
        };
        let summary_message = AgentMessage::compaction_summary(serialize_pi_mono_summary(&summary))
            .to_llm_message()
            .ok_or_else(|| {
                RociError::InvalidState("compaction summary message failed to convert".to_string())
            })?;

        let mut compacted = Vec::with_capacity(
            system_prefix.len()
                + 1
                + prepared.turn_prefix_messages.len()
                + prepared.kept_messages.len(),
        );
        compacted.extend(system_prefix);
        compacted.push(summary_message);
        compacted.extend(prepared.turn_prefix_messages);
        compacted.extend(prepared.kept_messages);
        Ok(Some(compacted))
    }

    /// Build an event sink that intercepts [`AgentEvent`]s to update tracking
    /// fields, broadcasts the snapshot, and forwards to the user-provided sink.
    fn build_intercepting_sink(&self) -> AgentEventSink {
        let original_sink = self.config.event_sink.clone();
        let turn_index = self.turn_index.clone();
        let is_streaming = self.is_streaming.clone();
        let messages = self.messages.clone();
        let last_error = self.last_error.clone();
        let state = self.state.clone();
        let snapshot_tx = self.snapshot_tx.clone();

        Arc::new(move |event: AgentEvent| {
            if let AgentEvent::TurnStart {
                turn_index: idx, ..
            } = &event
            {
                if let Ok(mut value) = turn_index.try_lock() {
                    *value = *idx;
                }
                let snapshot = AgentSnapshot {
                    state: state
                        .try_lock()
                        .map(|value| *value)
                        .unwrap_or(AgentState::Running),
                    turn_index: *idx,
                    message_count: messages.try_lock().map(|value| value.len()).unwrap_or(0),
                    is_streaming: is_streaming.try_lock().map(|value| *value).unwrap_or(true),
                    last_error: last_error
                        .try_lock()
                        .map(|value| value.clone())
                        .unwrap_or(None),
                };
                let _ = snapshot_tx.send(snapshot);
            }
            if let Some(ref sink) = original_sink {
                sink(event);
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ModelCapabilities;
    use crate::provider::{ModelProvider, ProviderFactory, ProviderResponse};
    use crate::tools::arguments::ToolArguments;
    use crate::tools::dynamic::{DynamicTool, DynamicToolProvider};
    use crate::tools::tool::ToolExecutionContext;
    use crate::tools::{AgentTool, AgentToolParameters};
    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use std::sync::{Arc, Mutex};

    fn test_registry() -> Arc<ProviderRegistry> {
        Arc::new(ProviderRegistry::new())
    }

    fn test_config() -> RociConfig {
        RociConfig::new()
    }

    struct SummaryFactory {
        provider_key: &'static str,
        summary_text: String,
        created_models: Arc<Mutex<Vec<String>>>,
    }

    impl SummaryFactory {
        fn new(
            provider_key: &'static str,
            summary_text: impl Into<String>,
            created_models: Arc<Mutex<Vec<String>>>,
        ) -> Self {
            Self {
                provider_key,
                summary_text: summary_text.into(),
                created_models,
            }
        }
    }

    impl ProviderFactory for SummaryFactory {
        fn provider_keys(&self) -> &[&str] {
            std::slice::from_ref(&self.provider_key)
        }

        fn create(
            &self,
            _config: &RociConfig,
            _provider_key: &str,
            model_id: &str,
        ) -> Result<Box<dyn ModelProvider>, RociError> {
            self.created_models
                .lock()
                .expect("created_models lock")
                .push(model_id.to_string());
            Ok(Box::new(SummaryProvider {
                provider_key: self.provider_key.to_string(),
                model_id: model_id.to_string(),
                summary_text: self.summary_text.clone(),
                capabilities: ModelCapabilities::default(),
            }))
        }

        fn parse_model(
            &self,
            _provider_key: &str,
            _model_id: &str,
        ) -> Option<Box<dyn std::any::Any + Send + Sync>> {
            None
        }
    }

    struct SummaryProvider {
        provider_key: String,
        model_id: String,
        summary_text: String,
        capabilities: ModelCapabilities,
    }

    #[async_trait]
    impl ModelProvider for SummaryProvider {
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
            Ok(ProviderResponse {
                text: self.summary_text.clone(),
                usage: crate::types::Usage::default(),
                tool_calls: Vec::new(),
                finish_reason: None,
                thinking: Vec::new(),
            })
        }

        async fn stream_text(
            &self,
            _request: &ProviderRequest,
        ) -> Result<BoxStream<'static, Result<crate::types::TextStreamDelta, RociError>>, RociError>
        {
            Err(RociError::UnsupportedOperation(
                "summary test provider does not stream".to_string(),
            ))
        }
    }

    fn test_agent_config() -> AgentConfig {
        let model: LanguageModel = "openai:gpt-4o".parse().unwrap();
        AgentConfig {
            model,
            system_prompt: None,
            tools: Vec::new(),
            dynamic_tool_providers: Vec::new(),
            settings: GenerationSettings::default(),
            transform_context: None,
            convert_to_llm: None,
            event_sink: None,
            session_id: None,
            steering_mode: QueueDrainMode::All,
            follow_up_mode: QueueDrainMode::All,
            transport: None,
            max_retry_delay_ms: None,
            get_api_key: None,
            compaction: CompactionSettings::default(),
        }
    }

    fn registry_with_summary_provider(
        provider_key: &'static str,
        summary_text: &str,
        created_models: Arc<Mutex<Vec<String>>>,
    ) -> Arc<ProviderRegistry> {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(SummaryFactory::new(
            provider_key,
            summary_text,
            created_models,
        )));
        Arc::new(registry)
    }

    fn dummy_tool(name: &str) -> Arc<dyn Tool> {
        Arc::new(AgentTool::new(
            name,
            "test tool",
            AgentToolParameters::empty(),
            |_args, _ctx| async move { Ok(serde_json::json!({ "ok": true })) },
        ))
    }

    struct MockDynamicToolProvider {
        tools: Vec<DynamicTool>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl MockDynamicToolProvider {
        fn new(tools: Vec<DynamicTool>) -> Self {
            Self {
                tools,
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl DynamicToolProvider for MockDynamicToolProvider {
        async fn list_tools(&self) -> Result<Vec<DynamicTool>, RociError> {
            Ok(self.tools.clone())
        }

        async fn execute_tool(
            &self,
            name: &str,
            _args: &ToolArguments,
            _ctx: &ToolExecutionContext,
        ) -> Result<serde_json::Value, RociError> {
            let mut calls = self
                .calls
                .lock()
                .expect("calls lock should not be poisoned");
            calls.push(name.to_string());
            Ok(serde_json::json!({ "ok": true }))
        }
    }

    #[tokio::test]
    async fn new_agent_starts_idle() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        assert_eq!(agent.state().await, AgentState::Idle);
    }

    #[tokio::test]
    async fn messages_starts_empty() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        assert!(agent.messages().await.is_empty());
    }

    #[tokio::test]
    async fn wait_for_idle_returns_immediately_when_idle() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        // Should return instantly — no run in flight.
        agent.wait_for_idle().await;
        assert_eq!(agent.state().await, AgentState::Idle);
    }

    #[tokio::test]
    async fn steer_queues_message() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        agent.steer("change direction").await;
        let queue = agent.steering_queue.lock().await;
        assert_eq!(queue.len(), 1);
    }

    #[tokio::test]
    async fn follow_up_queues_message() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        agent.follow_up("next step").await;
        let queue = agent.follow_up_queue.lock().await;
        assert_eq!(queue.len(), 1);
    }

    #[tokio::test]
    async fn abort_returns_false_when_idle() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        assert!(!agent.abort().await);
    }

    #[tokio::test]
    async fn reset_clears_all_state() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        // Seed some queue data.
        agent.steer("msg1").await;
        agent.follow_up("msg2").await;

        agent.reset().await;

        assert_eq!(agent.state().await, AgentState::Idle);
        assert!(agent.steering_queue.lock().await.is_empty());
        assert!(agent.follow_up_queue.lock().await.is_empty());
        assert!(agent.messages().await.is_empty());
    }

    #[tokio::test]
    async fn watch_state_returns_idle_initially() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        let rx = agent.watch_state();
        assert_eq!(*rx.borrow(), AgentState::Idle);
    }

    #[tokio::test]
    async fn set_system_prompt_rejects_when_running() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        *agent.state.lock().await = AgentState::Running;

        let err = agent.set_system_prompt("new prompt").await.unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
    }

    #[tokio::test]
    async fn set_model_rejects_when_running() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        *agent.state.lock().await = AgentState::Running;

        let model: LanguageModel = "openai:gpt-4o-mini".parse().unwrap();
        let err = agent.set_model(model).await.unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
    }

    #[tokio::test]
    async fn set_model_replaces_runtime_model_when_idle() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        let model: LanguageModel = "openai:gpt-4o-mini".parse().unwrap();

        agent.set_model(model.clone()).await.unwrap();
        assert_eq!(*agent.model.lock().await, model);
    }

    #[tokio::test]
    async fn set_and_clear_system_prompt_work_when_idle() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        agent
            .set_system_prompt("use concise replies")
            .await
            .unwrap();
        assert_eq!(
            agent.system_prompt.lock().await.clone(),
            Some("use concise replies".into())
        );

        agent.clear_system_prompt().await.unwrap();
        assert_eq!(agent.system_prompt.lock().await.clone(), None);
    }

    #[tokio::test]
    async fn replace_messages_rejects_when_not_idle() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        *agent.state.lock().await = AgentState::Aborting;

        let err = agent
            .replace_messages(vec![ModelMessage::user("replacement")])
            .await
            .unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
    }

    #[tokio::test]
    async fn replace_messages_updates_snapshot_and_history() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        let mut rx = agent.watch_snapshot();

        agent
            .replace_messages(vec![
                ModelMessage::system("system"),
                ModelMessage::user("hello"),
                ModelMessage::assistant("response"),
            ])
            .await
            .unwrap();

        rx.changed().await.unwrap();
        assert_eq!(agent.messages().await.len(), 3);
        assert_eq!(rx.borrow().message_count, 3);
    }

    #[tokio::test]
    async fn set_tools_rejects_when_running() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        *agent.state.lock().await = AgentState::Running;

        let err = agent.set_tools(vec![dummy_tool("t1")]).await.unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
    }

    #[tokio::test]
    async fn set_tools_replaces_runtime_tool_registry() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        agent
            .set_tools(vec![dummy_tool("t1"), dummy_tool("t2")])
            .await
            .unwrap();

        let names: Vec<String> = agent
            .tools
            .lock()
            .await
            .iter()
            .map(|tool| tool.name().to_string())
            .collect();
        assert_eq!(names, vec!["t1".to_string(), "t2".to_string()]);
    }

    #[tokio::test]
    async fn resolve_tools_for_run_merges_static_and_dynamic_tools() {
        let provider: Arc<dyn DynamicToolProvider> =
            Arc::new(MockDynamicToolProvider::new(vec![DynamicTool {
                name: "dynamic".into(),
                description: "dynamic tool".into(),
                parameters: AgentToolParameters::empty(),
            }]));

        let mut config = test_agent_config();
        config.tools = vec![dummy_tool("static")];
        config.dynamic_tool_providers = vec![Arc::clone(&provider)];

        let agent = AgentRuntime::new(test_registry(), test_config(), config);

        let tools = agent
            .resolve_tools_for_run()
            .await
            .expect("tools should resolve");
        let names = tools
            .iter()
            .map(|tool| tool.name().to_string())
            .collect::<Vec<_>>();

        assert!(names.contains(&"static".to_string()));
        assert!(names.contains(&"dynamic".to_string()));
    }

    #[tokio::test]
    async fn set_dynamic_tool_providers_replaces_runtime_registry() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        let provider: Arc<dyn DynamicToolProvider> =
            Arc::new(MockDynamicToolProvider::new(Vec::new()));

        agent
            .set_dynamic_tool_providers(vec![Arc::clone(&provider)])
            .await
            .expect("dynamic providers should be replaced");

        let providers = agent.dynamic_tool_providers.lock().await;
        assert_eq!(providers.len(), 1);
    }

    #[tokio::test]
    async fn clear_dynamic_tool_providers_empties_registry() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        let provider: Arc<dyn DynamicToolProvider> =
            Arc::new(MockDynamicToolProvider::new(Vec::new()));

        agent
            .set_dynamic_tool_providers(vec![Arc::clone(&provider)])
            .await
            .expect("dynamic providers should be set");

        agent
            .clear_dynamic_tool_providers()
            .await
            .expect("dynamic providers should be cleared");

        let providers = agent.dynamic_tool_providers.lock().await;
        assert!(providers.is_empty());
    }

    #[tokio::test]
    async fn clear_queue_apis_and_has_queued_messages_behave_consistently() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        assert!(!agent.has_queued_messages().await);

        agent.steer("s1").await;
        assert!(agent.has_queued_messages().await);
        assert_eq!(agent.steering_queue.lock().await.len(), 1);
        assert_eq!(agent.follow_up_queue.lock().await.len(), 0);

        agent.clear_steering_queue().await;
        assert!(!agent.has_queued_messages().await);
        assert!(agent.steering_queue.lock().await.is_empty());

        agent.follow_up("f1").await;
        agent.follow_up("f2").await;
        assert!(agent.has_queued_messages().await);
        assert_eq!(agent.follow_up_queue.lock().await.len(), 2);

        agent.clear_follow_up_queue().await;
        assert!(!agent.has_queued_messages().await);
        assert!(agent.follow_up_queue.lock().await.is_empty());

        agent.steer("s2").await;
        agent.follow_up("f3").await;
        assert!(agent.has_queued_messages().await);

        agent.clear_all_queues().await;
        assert!(!agent.has_queued_messages().await);
        assert!(agent.steering_queue.lock().await.is_empty());
        assert!(agent.follow_up_queue.lock().await.is_empty());
    }

    #[tokio::test]
    async fn clearing_queues_restores_continue_without_input_assistant_guard() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        agent
            .messages
            .lock()
            .await
            .push(ModelMessage::assistant("done"));
        agent.steer("queued steer").await;

        assert!(agent.has_queued_messages().await);
        agent.clear_all_queues().await;
        assert!(!agent.has_queued_messages().await);

        let err = agent.continue_without_input().await.unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
        assert_eq!(
            err.to_string(),
            "Invalid state: Cannot continue from message role: assistant"
        );
    }

    #[tokio::test]
    async fn transition_to_running_fails_when_not_idle() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        // Force state to Running manually to test the guard.
        *agent.state.lock().await = AgentState::Running;

        let err = agent.transition_to_running().unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
    }

    #[tokio::test]
    async fn prompt_with_system_prepends_system_message() {
        let config = AgentConfig {
            system_prompt: Some("You are helpful.".into()),
            ..test_agent_config()
        };
        let agent = AgentRuntime::new(test_registry(), test_config(), config);

        // We can't actually run the loop (no real provider), but we can verify
        // the transition_to_running guard works and then manually check message assembly.
        // Directly test the message assembly logic:
        {
            let system_prompt = agent.system_prompt.lock().await.clone();
            let mut msgs = agent.messages.lock().await;
            if let Some(ref sys) = system_prompt {
                if msgs.is_empty() {
                    msgs.push(ModelMessage::system(sys.clone()));
                }
            }
            msgs.push(ModelMessage::user("hello"));
        }

        let msgs = agent.messages().await;
        assert_eq!(msgs.len(), 2);
        // First message should be the system prompt.
        assert_eq!(msgs[0].role, crate::types::Role::System);
        assert_eq!(msgs[1].role, crate::types::Role::User);
    }

    #[tokio::test]
    async fn continue_run_rejects_when_running() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        // Force state to Running.
        *agent.state.lock().await = AgentState::Running;

        let err = agent.continue_run("more").await.unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
    }

    #[tokio::test]
    async fn continue_without_input_rejects_when_running() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        *agent.state.lock().await = AgentState::Running;

        let err = agent.continue_without_input().await.unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
    }

    #[tokio::test]
    async fn continue_without_input_rejects_when_history_is_empty() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        let err = agent.continue_without_input().await.unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
        assert_eq!(
            err.to_string(),
            "Invalid state: No messages to continue from"
        );
        assert_eq!(agent.state().await, AgentState::Idle);
    }

    #[tokio::test]
    async fn continue_without_input_rejects_from_assistant_without_queued_messages() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        agent
            .messages
            .lock()
            .await
            .push(ModelMessage::assistant("done"));

        let err = agent.continue_without_input().await.unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
        assert_eq!(
            err.to_string(),
            "Invalid state: Cannot continue from message role: assistant"
        );
        assert_eq!(agent.state().await, AgentState::Idle);
    }

    #[tokio::test]
    async fn prompt_rejects_when_aborting() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        *agent.state.lock().await = AgentState::Aborting;

        let err = agent.prompt("hey").await.unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
    }

    #[tokio::test]
    async fn multiple_steers_accumulate() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        agent.steer("a").await;
        agent.steer("b").await;
        agent.steer("c").await;
        assert_eq!(agent.steering_queue.lock().await.len(), 3);
    }

    #[tokio::test]
    async fn multiple_follow_ups_accumulate() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        agent.follow_up("x").await;
        agent.follow_up("y").await;
        assert_eq!(agent.follow_up_queue.lock().await.len(), 2);
    }

    #[test]
    fn agent_state_equality() {
        assert_eq!(AgentState::Idle, AgentState::Idle);
        assert_ne!(AgentState::Idle, AgentState::Running);
        assert_ne!(AgentState::Running, AgentState::Aborting);
    }

    #[test]
    fn agent_state_debug() {
        let s = format!("{:?}", AgentState::Running);
        assert_eq!(s, "Running");
    }

    #[test]
    fn agent_state_clone_copy() {
        let s = AgentState::Idle;
        let s2 = s; // Copy
        let s3 = s.clone(); // Clone
        assert_eq!(s, s2);
        assert_eq!(s2, s3);
    }

    #[test]
    fn queue_drain_mode_all_drains_everything() {
        let mut queue = vec![ModelMessage::user("one"), ModelMessage::user("two")];
        let drained = drain_queue(&mut queue, QueueDrainMode::All);
        assert_eq!(drained.len(), 2);
        assert!(queue.is_empty());
    }

    #[test]
    fn queue_drain_mode_one_at_a_time_drains_incrementally() {
        let mut queue = vec![
            ModelMessage::user("one"),
            ModelMessage::user("two"),
            ModelMessage::user("three"),
        ];
        let first = drain_queue(&mut queue, QueueDrainMode::OneAtATime);
        assert_eq!(first.len(), 1);
        assert_eq!(queue.len(), 2);
        let second = drain_queue(&mut queue, QueueDrainMode::OneAtATime);
        assert_eq!(second.len(), 1);
        assert_eq!(queue.len(), 1);
    }

    // -- Lifecycle control tests --

    #[tokio::test]
    async fn abort_is_idempotent_when_idle() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        assert!(!agent.abort().await);
        assert!(!agent.abort().await);
        assert_eq!(agent.state().await, AgentState::Idle);
    }

    #[tokio::test]
    async fn reset_is_idempotent() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        agent.reset().await;
        assert_eq!(agent.state().await, AgentState::Idle);

        agent.reset().await;
        assert_eq!(agent.state().await, AgentState::Idle);
    }

    #[tokio::test]
    async fn reset_clears_queued_steering_and_follow_up_messages() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        agent.steer("steer-1").await;
        agent.steer("steer-2").await;
        agent.follow_up("follow-1").await;
        agent.follow_up("follow-2").await;
        agent.follow_up("follow-3").await;

        assert_eq!(agent.steering_queue.lock().await.len(), 2);
        assert_eq!(agent.follow_up_queue.lock().await.len(), 3);

        agent.reset().await;

        assert!(agent.steering_queue.lock().await.is_empty());
        assert!(agent.follow_up_queue.lock().await.is_empty());
    }

    #[tokio::test]
    async fn watch_state_reflects_manual_transitions() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        let mut rx = agent.watch_state();

        assert_eq!(*rx.borrow(), AgentState::Idle);

        {
            let mut state = agent.state.lock().await;
            *state = AgentState::Running;
            let _ = agent.state_tx.send(AgentState::Running);
        }

        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), AgentState::Running);
    }

    #[tokio::test]
    async fn multiple_aborts_are_safe() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        *agent.state.lock().await = AgentState::Running;
        let _ = agent.state_tx.send(AgentState::Running);

        let first = agent.abort().await;
        assert_eq!(agent.state().await, AgentState::Aborting);
        // Returns false because there is no active RunHandle in a test context,
        // but state has transitioned to Aborting regardless.
        assert!(!first);

        let second = agent.abort().await;
        assert!(!second);
        assert_eq!(agent.state().await, AgentState::Aborting);
    }

    #[tokio::test]
    async fn continue_run_rejects_during_aborting() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        *agent.state.lock().await = AgentState::Aborting;

        let err = agent.continue_run("more input").await.unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
    }

    #[tokio::test]
    async fn continue_without_input_rejects_during_aborting() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        *agent.state.lock().await = AgentState::Aborting;

        let err = agent.continue_without_input().await.unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
    }

    // -- AgentSnapshot tests --

    #[tokio::test]
    async fn snapshot_starts_with_idle_defaults() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        let snap = agent.snapshot().await;

        assert_eq!(snap.state, AgentState::Idle);
        assert_eq!(snap.turn_index, 0);
        assert_eq!(snap.message_count, 0);
        assert!(!snap.is_streaming);
        assert_eq!(snap.last_error, None);
    }

    #[tokio::test]
    async fn watch_snapshot_returns_receiver() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        let rx1 = agent.watch_snapshot();
        let rx2 = rx1.clone();

        let snap = rx1.borrow().clone();
        assert_eq!(snap.state, AgentState::Idle);
        assert_eq!(snap.turn_index, 0);
        assert_eq!(snap.message_count, 0);
        assert!(!snap.is_streaming);
        assert_eq!(snap.last_error, None);

        assert_eq!(*rx2.borrow(), snap);
    }

    #[tokio::test]
    async fn snapshot_reflects_queued_messages() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        assert_eq!(agent.snapshot().await.message_count, 0);

        agent
            .messages
            .lock()
            .await
            .push(ModelMessage::user("hello"));
        assert_eq!(agent.snapshot().await.message_count, 1);

        agent
            .messages
            .lock()
            .await
            .push(ModelMessage::user("follow up"));
        assert_eq!(agent.snapshot().await.message_count, 2);
    }

    #[tokio::test]
    async fn snapshot_reflects_state_changes() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        *agent.state.lock().await = AgentState::Running;
        assert_eq!(agent.snapshot().await.state, AgentState::Running);

        *agent.state.lock().await = AgentState::Aborting;
        assert_eq!(agent.snapshot().await.state, AgentState::Aborting);
    }

    #[tokio::test]
    async fn reset_clears_snapshot_fields() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

        *agent.turn_index.lock().await = 5;
        *agent.last_error.lock().await = Some("boom".into());
        agent.messages.lock().await.push(ModelMessage::user("msg"));

        agent.reset().await;

        let snap = agent.snapshot().await;
        assert_eq!(snap.state, AgentState::Idle);
        assert_eq!(snap.turn_index, 0);
        assert_eq!(snap.message_count, 0);
        assert!(!snap.is_streaming);
        assert_eq!(snap.last_error, None);
    }

    #[tokio::test]
    async fn watch_snapshot_notifies_on_reset() {
        let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
        let mut rx = agent.watch_snapshot();

        *agent.turn_index.lock().await = 3;
        *agent.last_error.lock().await = Some("err".into());

        agent.reset().await;

        rx.changed().await.unwrap();
        let snap = rx.borrow().clone();
        assert_eq!(snap.state, AgentState::Idle);
        assert_eq!(snap.turn_index, 0);
        assert_eq!(snap.last_error, None);
    }

    #[test]
    fn agent_snapshot_debug_and_clone() {
        let snap = AgentSnapshot {
            state: AgentState::Running,
            turn_index: 2,
            message_count: 5,
            is_streaming: true,
            last_error: Some("test error".into()),
        };
        let cloned = snap.clone();
        assert_eq!(snap, cloned);
        let debug = format!("{:?}", snap);
        assert!(debug.contains("Running"));
        assert!(debug.contains("test error"));
    }

    // -- GetApiKeyFn callback tests --

    #[tokio::test]
    async fn get_api_key_callback_returns_resolved_key() {
        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = call_count.clone();

        let get_key: GetApiKeyFn = Arc::new(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok("sk-live-rotated-key".to_string()) })
        });

        let key = get_key().await.unwrap();
        assert_eq!(key, "sk-live-rotated-key");
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);

        let key2 = get_key().await.unwrap();
        assert_eq!(key2, "sk-live-rotated-key");
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn static_key_works_without_callback() {
        let config = AgentConfig {
            get_api_key: None,
            ..test_agent_config()
        };
        let agent = AgentRuntime::new(test_registry(), test_config(), config);

        assert!(agent.config.get_api_key.is_none());
        assert_eq!(agent.state().await, AgentState::Idle);
    }

    #[tokio::test]
    async fn get_api_key_error_propagates() {
        let get_key: GetApiKeyFn = Arc::new(|| {
            Box::pin(async {
                Err(RociError::Authentication(
                    "Token refresh failed".to_string(),
                ))
            })
        });

        let result = get_key().await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RociError::Authentication(msg) if msg == "Token refresh failed"
        ));
    }

    #[tokio::test]
    async fn prompt_get_api_key_error_restores_idle_state() {
        let get_key: GetApiKeyFn = Arc::new(|| {
            Box::pin(async {
                Err(RociError::Authentication(
                    "Token refresh failed".to_string(),
                ))
            })
        });
        let agent = AgentRuntime::new(
            test_registry(),
            test_config(),
            AgentConfig {
                get_api_key: Some(get_key),
                ..test_agent_config()
            },
        );

        let err = agent.prompt("hello").await.unwrap_err();
        assert!(matches!(
            err,
            RociError::Authentication(msg) if msg == "Token refresh failed"
        ));
        assert_eq!(agent.state().await, AgentState::Idle);

        // Must not block after a failed prompt.
        agent.wait_for_idle().await;

        let snap = agent.snapshot().await;
        assert_eq!(snap.state, AgentState::Idle);
        assert!(!snap.is_streaming);
        assert_eq!(
            snap.last_error,
            Some("Authentication error: Token refresh failed".into())
        );
    }

    #[tokio::test]
    async fn agent_runtime_uses_config_api_key_by_default() {
        let roci_config = RociConfig::new().with_token_store(None);
        roci_config.set_api_key("openai", "sk-from-config".to_string());

        let agent_config = AgentConfig {
            get_api_key: None,
            ..test_agent_config()
        };
        let agent = AgentRuntime::new(test_registry(), roci_config, agent_config);

        assert!(agent.config.get_api_key.is_none());
        assert_eq!(agent.state().await, AgentState::Idle);
    }

    #[tokio::test]
    async fn get_api_key_callback_can_rotate_keys() {
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter_clone = counter.clone();

        let get_key: GetApiKeyFn = Arc::new(move || {
            let n = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move { Ok(format!("sk-key-{}", n)) })
        });

        assert_eq!(get_key().await.unwrap(), "sk-key-0");
        assert_eq!(get_key().await.unwrap(), "sk-key-1");
        assert_eq!(get_key().await.unwrap(), "sk-key-2");
    }

    #[tokio::test]
    async fn compact_replaces_history_with_summary_and_preserves_system_prompt() {
        let created_models = Arc::new(Mutex::new(Vec::new()));
        let registry = registry_with_summary_provider("stub", "summarized context", created_models);
        let mut config = test_agent_config();
        config.model = "stub:run-model".parse().expect("stub model should parse");
        config.compaction.keep_recent_tokens = 1;
        let agent = AgentRuntime::new(registry, test_config(), config);

        agent
            .replace_messages(vec![
                ModelMessage::system("You are precise"),
                ModelMessage::user("first"),
                ModelMessage::assistant("answer"),
                ModelMessage::user("latest"),
            ])
            .await
            .expect("messages should be set");

        agent
            .compact()
            .await
            .expect("manual compaction should succeed");
        let messages = agent.messages().await;

        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].text(), "You are precise");
        assert!(
            messages
                .iter()
                .any(|message| message.text().contains("<compaction_summary>")),
            "compacted history should include a summary wrapper"
        );
        assert!(
            messages.len() < 4,
            "manual compaction should replace part of the history"
        );
    }

    #[tokio::test]
    async fn compact_uses_configured_compaction_model_when_present() {
        let created_models = Arc::new(Mutex::new(Vec::new()));
        let registry = registry_with_summary_provider("summary", "summary", created_models.clone());
        let mut config = test_agent_config();
        config.model = "run:model".parse().expect("run model should parse");
        config.compaction.model = Some("summary:compact-model".to_string());
        config.compaction.keep_recent_tokens = 1;
        let agent = AgentRuntime::new(registry, test_config(), config);

        agent
            .replace_messages(vec![
                ModelMessage::user("first"),
                ModelMessage::assistant("second"),
                ModelMessage::user("third"),
            ])
            .await
            .expect("messages should be set");

        agent.compact().await.expect("compaction should succeed");

        let created_models = created_models.lock().expect("created models lock");
        assert_eq!(created_models.as_slice(), ["compact-model"]);
    }
}
