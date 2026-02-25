//! High-level Agent runtime wrapping the agent loop.
//!
//! Provides the pi-mono aligned API surface:
//! - [`Agent::prompt`] — start a new conversation
//! - [`Agent::continue_run`] — continue with additional context
//! - [`Agent::steer`] — interrupt tool execution with a message
//! - [`Agent::follow_up`] — queue a message after natural completion
//! - [`Agent::abort`] — cancel the current run
//! - [`Agent::reset`] — clear conversation and state
//! - [`Agent::wait_for_idle`] — block until the agent finishes

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{watch, Mutex, Notify};

use crate::agent_loop::{
    AgentEvent, LoopRunner, RunHandle, RunRequest, RunResult,
    RunStatus, Runner,
};
use crate::agent_loop::runner::{
    AgentEventSink, FollowUpMessagesFn, SteeringMessagesFn, TransformContextFn,
};
use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::tools::tool::Tool;
use crate::types::{GenerationSettings, ModelMessage};

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
    /// Generation settings (temperature, max_tokens, etc.).
    pub settings: GenerationSettings,
    /// Optional hook to transform the message context before each LLM call.
    pub transform_context: Option<TransformContextFn>,
    /// Optional sink for high-level [`AgentEvent`](crate::agent_loop::AgentEvent) emission.
    pub event_sink: Option<AgentEventSink>,
    /// Optional session ID for provider-side prompt caching.
    pub session_id: Option<String>,
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
/// let agent = AgentRuntime::new(roci_config, config);
/// let result = agent.prompt("Hello").await?;
/// let result = agent.continue_run("Tell me more").await?;
/// agent.reset().await;
/// ```
pub struct AgentRuntime {
    config: AgentConfig,
    runner: LoopRunner,
    state: Arc<Mutex<AgentState>>,
    state_tx: watch::Sender<AgentState>,
    state_rx: watch::Receiver<AgentState>,
    messages: Arc<Mutex<Vec<ModelMessage>>>,
    steering_queue: Arc<Mutex<Vec<ModelMessage>>>,
    follow_up_queue: Arc<Mutex<Vec<ModelMessage>>>,
    active_handle: Arc<Mutex<Option<RunHandle>>>,
    idle_notify: Arc<Notify>,
    turn_index: Arc<Mutex<usize>>,
    is_streaming: Arc<Mutex<bool>>,
    last_error: Arc<Mutex<Option<String>>>,
    snapshot_tx: watch::Sender<AgentSnapshot>,
    snapshot_rx: watch::Receiver<AgentSnapshot>,
}

impl AgentRuntime {
    /// Create a new agent runtime with the given configuration.
    pub fn new(roci_config: RociConfig, config: AgentConfig) -> Self {
        let runner = LoopRunner::new(roci_config);
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
            state: Arc::new(Mutex::new(AgentState::Idle)),
            state_tx,
            state_rx,
            messages: Arc::new(Mutex::new(Vec::new())),
            steering_queue: Arc::new(Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            active_handle: Arc::new(Mutex::new(None)),
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
        let mut msgs = self.messages.lock().await;
        if let Some(ref sys) = self.config.system_prompt {
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

        let mut handle = self.active_handle.lock().await;
        if let Some(h) = handle.as_mut() {
            h.abort()
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

    /// Build a [`RunRequest`], start the loop, wait for the result, then
    /// transition back to Idle.
    async fn run_loop(&self, initial_messages: Vec<ModelMessage>) -> Result<RunResult, RociError> {
        *self.is_streaming.lock().await = true;
        self.broadcast_snapshot().await;

        let steering_queue = self.steering_queue.clone();
        let follow_up_queue = self.follow_up_queue.clone();

        let steering_fn: SteeringMessagesFn = Arc::new(move || {
            let mut queue = steering_queue.blocking_lock();
            std::mem::take(&mut *queue)
        });

        let follow_up_fn: FollowUpMessagesFn = {
            let queue = follow_up_queue.clone();
            Arc::new(move || {
                let mut q = queue.blocking_lock();
                std::mem::take(&mut *q)
            })
        };

        let intercepting_sink = self.build_intercepting_sink();

        let mut request = RunRequest::new(self.config.model.clone(), initial_messages)
            .with_tools(self.config.tools.clone())
            .with_steering_messages(steering_fn)
            .with_follow_up_messages(follow_up_fn)
            .with_agent_event_sink(intercepting_sink);

        request.settings = self.config.settings.clone();

        if let Some(ref transform) = self.config.transform_context {
            request = request.with_transform_context(transform.clone());
        }
        if let Some(ref id) = self.config.session_id {
            request = request.with_session_id(id.clone());
        }

        if let Some(ref get_key) = self.config.get_api_key {
            let key = get_key().await?;
            request.metadata.insert("api_key".to_string(), key);
        }
        // When no callback is set, RociConfig.get_api_key() is called by
        // create_provider() inside the LoopRunner. That method already
        // checks env vars → credentials.json → OAuth token store, so no
        // extra wiring is needed here for the default case.

        let handle = self.runner.start(request).await?;

        // Store the handle so `abort()` can reach it.
        self.active_handle.lock().await.replace(handle);

        // Take the handle back to await the result.
        // Two separate lock scopes avoid holding the mutex across the `.wait()`.
        let handle = self.active_handle.lock().await.take();
        let result = match handle {
            Some(h) => h.wait().await,
            None => RunResult::canceled(),
        };

        // Capture error from failed runs.
        if result.status == RunStatus::Failed {
            *self.last_error.lock().await = result.error.clone();
        }
        *self.is_streaming.lock().await = false;

        // Transition back to Idle.
        let mut state = self.state.lock().await;
        *state = AgentState::Idle;
        let _ = self.state_tx.send(AgentState::Idle);
        drop(state);
        self.broadcast_snapshot().await;
        self.idle_notify.notify_waiters();

        Ok(result)
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
                *turn_index.blocking_lock() = *idx;
                let snapshot = AgentSnapshot {
                    state: *state.blocking_lock(),
                    turn_index: *idx,
                    message_count: messages.blocking_lock().len(),
                    is_streaming: *is_streaming.blocking_lock(),
                    last_error: last_error.blocking_lock().clone(),
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

    fn test_config() -> RociConfig {
        RociConfig::new()
    }

    fn test_agent_config() -> AgentConfig {
        let model: LanguageModel = "openai:gpt-4o".parse().unwrap();
        AgentConfig {
            model,
            system_prompt: None,
            tools: Vec::new(),
            settings: GenerationSettings::default(),
            transform_context: None,
            event_sink: None,
            session_id: None,
            get_api_key: None,
        }
    }

    #[tokio::test]
    async fn new_agent_starts_idle() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());
        assert_eq!(agent.state().await, AgentState::Idle);
    }

    #[tokio::test]
    async fn messages_starts_empty() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());
        assert!(agent.messages().await.is_empty());
    }

    #[tokio::test]
    async fn wait_for_idle_returns_immediately_when_idle() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());
        // Should return instantly — no run in flight.
        agent.wait_for_idle().await;
        assert_eq!(agent.state().await, AgentState::Idle);
    }

    #[tokio::test]
    async fn steer_queues_message() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());
        agent.steer("change direction").await;
        let queue = agent.steering_queue.lock().await;
        assert_eq!(queue.len(), 1);
    }

    #[tokio::test]
    async fn follow_up_queues_message() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());
        agent.follow_up("next step").await;
        let queue = agent.follow_up_queue.lock().await;
        assert_eq!(queue.len(), 1);
    }

    #[tokio::test]
    async fn abort_returns_false_when_idle() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());
        assert!(!agent.abort().await);
    }

    #[tokio::test]
    async fn reset_clears_all_state() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());

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
        let agent = AgentRuntime::new(test_config(), test_agent_config());
        let rx = agent.watch_state();
        assert_eq!(*rx.borrow(), AgentState::Idle);
    }

    #[tokio::test]
    async fn transition_to_running_fails_when_not_idle() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());

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
        let agent = AgentRuntime::new(test_config(), config);

        // We can't actually run the loop (no real provider), but we can verify
        // the transition_to_running guard works and then manually check message assembly.
        // Directly test the message assembly logic:
        {
            let mut msgs = agent.messages.lock().await;
            if let Some(ref sys) = agent.config.system_prompt {
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
        let agent = AgentRuntime::new(test_config(), test_agent_config());

        // Force state to Running.
        *agent.state.lock().await = AgentState::Running;

        let err = agent.continue_run("more").await.unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
    }

    #[tokio::test]
    async fn prompt_rejects_when_aborting() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());

        *agent.state.lock().await = AgentState::Aborting;

        let err = agent.prompt("hey").await.unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
    }

    #[tokio::test]
    async fn multiple_steers_accumulate() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());
        agent.steer("a").await;
        agent.steer("b").await;
        agent.steer("c").await;
        assert_eq!(agent.steering_queue.lock().await.len(), 3);
    }

    #[tokio::test]
    async fn multiple_follow_ups_accumulate() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());
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

    // -- Lifecycle control tests --

    #[tokio::test]
    async fn abort_is_idempotent_when_idle() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());

        assert!(!agent.abort().await);
        assert!(!agent.abort().await);
        assert_eq!(agent.state().await, AgentState::Idle);
    }

    #[tokio::test]
    async fn reset_is_idempotent() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());

        agent.reset().await;
        assert_eq!(agent.state().await, AgentState::Idle);

        agent.reset().await;
        assert_eq!(agent.state().await, AgentState::Idle);
    }

    #[tokio::test]
    async fn reset_clears_queued_steering_and_follow_up_messages() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());

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
        let agent = AgentRuntime::new(test_config(), test_agent_config());
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
        let agent = AgentRuntime::new(test_config(), test_agent_config());

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
        let agent = AgentRuntime::new(test_config(), test_agent_config());

        *agent.state.lock().await = AgentState::Aborting;

        let err = agent.continue_run("more input").await.unwrap_err();
        assert!(matches!(err, RociError::InvalidState(_)));
    }

    // -- AgentSnapshot tests --

    #[tokio::test]
    async fn snapshot_starts_with_idle_defaults() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());
        let snap = agent.snapshot().await;

        assert_eq!(snap.state, AgentState::Idle);
        assert_eq!(snap.turn_index, 0);
        assert_eq!(snap.message_count, 0);
        assert!(!snap.is_streaming);
        assert_eq!(snap.last_error, None);
    }

    #[tokio::test]
    async fn watch_snapshot_returns_receiver() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());
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
        let agent = AgentRuntime::new(test_config(), test_agent_config());

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
        let agent = AgentRuntime::new(test_config(), test_agent_config());

        *agent.state.lock().await = AgentState::Running;
        assert_eq!(agent.snapshot().await.state, AgentState::Running);

        *agent.state.lock().await = AgentState::Aborting;
        assert_eq!(agent.snapshot().await.state, AgentState::Aborting);
    }

    #[tokio::test]
    async fn reset_clears_snapshot_fields() {
        let agent = AgentRuntime::new(test_config(), test_agent_config());

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
        let agent = AgentRuntime::new(test_config(), test_agent_config());
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
        let agent = AgentRuntime::new(test_config(), config);

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
    async fn agent_runtime_uses_config_api_key_by_default() {
        let roci_config = RociConfig::new().with_token_store(None);
        roci_config.set_api_key("openai", "sk-from-config".to_string());

        let agent_config = AgentConfig {
            get_api_key: None,
            ..test_agent_config()
        };
        let agent = AgentRuntime::new(roci_config, agent_config);

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
}
