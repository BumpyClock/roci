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
use tokio_util::sync::CancellationToken;

mod config;
mod summary;
mod types;

pub use self::config::AgentConfig;
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
use crate::agent_loop::runner::{
    AgentEventSink, AutoCompactionConfig, BeforeAgentStartHookPayload, BeforeAgentStartHookResult,
    CompactionHandler, FollowUpMessagesFn, RunHooks, SteeringMessagesFn,
};
use crate::agent_loop::{
    AgentEvent, LoopRunner, RunHandle, RunRequest, RunResult, RunStatus, Runner,
};
use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider::ProviderRegistry;
#[cfg(test)]
use crate::provider::ProviderRequest;
#[cfg(test)]
use crate::resource::{BranchSummarySettings, CompactionSettings};
use crate::tools::dynamic::{DynamicToolAdapter, DynamicToolProvider};
use crate::tools::tool::Tool;
#[cfg(test)]
use crate::types::GenerationSettings;
use crate::types::{ModelMessage, Role};

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

        if let Some(hook) = self.config.before_agent_start.clone() {
            let hook_cancel_token = CancellationToken::new();
            let hook_payload = BeforeAgentStartHookPayload {
                run_id: request.run_id,
                model: request.model.clone(),
                messages: request.messages.clone(),
                cancellation_token: hook_cancel_token.clone(),
            };
            match hook(hook_payload).await {
                Ok(BeforeAgentStartHookResult::Continue) => {}
                Ok(BeforeAgentStartHookResult::ReplaceMessages { messages }) => {
                    request.messages = messages;
                }
                Ok(BeforeAgentStartHookResult::Cancel { .. }) => {
                    self.restore_idle_after_preflight_error().await;
                    return Ok(RunResult::canceled_with_messages(request.messages.clone()));
                }
                Err(err) => {
                    self.restore_idle_after_preflight_error().await;
                    return Err(RociError::InvalidState(format!(
                        "before_agent_start hook failed: {err}"
                    )));
                }
            }
            hook_cancel_token.cancel();
        }

        let mut run_hooks = RunHooks {
            compaction: None,
            pre_tool_use: self.config.pre_tool_use.clone(),
            post_tool_use: self.config.post_tool_use.clone(),
        };

        if self.config.compaction.enabled {
            let compaction_settings = self.config.compaction.clone();
            let session_before_compact = self.config.session_before_compact.clone();
            let registry = self.registry.clone();
            let roci_config = self.roci_config.clone();
            let run_model = request.model.clone();
            let compaction_hook: CompactionHandler = Arc::new(move |messages, _cancel| {
                let compaction_settings = compaction_settings.clone();
                let session_before_compact = session_before_compact.clone();
                let registry = registry.clone();
                let roci_config = roci_config.clone();
                let run_model = run_model.clone();
                Box::pin(async move {
                    AgentRuntime::compact_messages_with_model(
                        messages,
                        &run_model,
                        &compaction_settings,
                        session_before_compact.as_ref(),
                        &registry,
                        &roci_config,
                    )
                    .await
                })
            });
            run_hooks.compaction = Some(compaction_hook);
            request = request.with_auto_compaction(AutoCompactionConfig {
                reserve_tokens: self.config.compaction.reserve_tokens,
            });
        }
        request = request.with_hooks(run_hooks);

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
        request = request.with_retry_backoff(self.config.retry_backoff);
        if let Some(ref api_key_override) = self.config.api_key_override {
            request = request.with_api_key_override(api_key_override.clone());
        }
        if !self.config.provider_headers.is_empty() {
            request = request.with_provider_headers(self.config.provider_headers.clone());
        }
        if !self.config.provider_metadata.is_empty() {
            request = request.with_provider_metadata(self.config.provider_metadata.clone());
        }
        if let Some(ref callback) = self.config.provider_payload_callback {
            request = request.with_provider_payload_callback(callback.clone());
        }

        let run_result = async {
            let provider_has_config_key = self
                .roci_config
                .get_api_key(request.model.provider_name())
                .is_some();
            if request.api_key_override.is_none() && !provider_has_config_key {
                if let Some(ref get_key) = self.config.get_api_key {
                    let key = get_key().await?;
                    request = request.with_api_key_override(key);
                }
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
#[path = "runtime_tests/mod.rs"]
mod tests;
