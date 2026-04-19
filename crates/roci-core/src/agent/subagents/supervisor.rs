//! Sub-agent supervisor: lifecycle management, concurrency, and event forwarding.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, oneshot, watch, Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::runtime::AgentConfig;
#[cfg(feature = "agent")]
use crate::agent::runtime::UserInputCoordinator;
use crate::agent_loop::RunStatus;
use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider::ProviderRegistry;

use super::context::build_child_initial_messages;
use super::handle::SubagentHandle;
use super::launcher::{InProcessLauncher, SubagentLauncher};
use super::profiles::SubagentProfileRegistry;
use super::prompt::SubagentPromptPolicy;
use super::types::{
    SubagentCompletion, SubagentContext, SubagentEvent, SubagentId, SubagentRunResult,
    SubagentSnapshot, SubagentSpec, SubagentStatus, SubagentSummary, SubagentSupervisorConfig,
};

#[cfg(test)]
use super::launcher::LaunchedChild;

// ---------------------------------------------------------------------------
// Internal child tracking
// ---------------------------------------------------------------------------

struct ChildEntry {
    id: SubagentId,
    label: Option<String>,
    profile: String,
    model: Option<LanguageModel>,
    status: Arc<Mutex<SubagentStatus>>,
    cancel_token: CancellationToken,
}

// ---------------------------------------------------------------------------
// Supervisor
// ---------------------------------------------------------------------------

/// Manages the lifecycle of child sub-agent runtimes.
///
/// Responsibilities:
/// - Profile resolution and model selection
/// - Concurrency limiting via semaphore
/// - Event forwarding from children to parent
/// - Abort, wait, and shutdown orchestration
pub struct SubagentSupervisor {
    registry: Arc<ProviderRegistry>,
    roci_config: RociConfig,
    config: SubagentSupervisorConfig,
    profile_registry: SubagentProfileRegistry,
    prompt_policy: SubagentPromptPolicy,
    base_config: AgentConfig,
    launcher: Box<dyn SubagentLauncher>,
    #[cfg(feature = "agent")]
    coordinator: Arc<UserInputCoordinator>,
    event_tx: broadcast::Sender<SubagentEvent>,
    children: Arc<Mutex<HashMap<SubagentId, ChildEntry>>>,
    concurrency_semaphore: Arc<Semaphore>,
}

impl SubagentSupervisor {
    /// Create a new supervisor.
    ///
    /// `base_config` provides the default tools and settings inherited by
    /// children unless overridden by their profile.
    pub fn new(
        registry: Arc<ProviderRegistry>,
        roci_config: RociConfig,
        base_config: AgentConfig,
        supervisor_config: SubagentSupervisorConfig,
        profile_registry: SubagentProfileRegistry,
    ) -> Self {
        #[cfg(feature = "agent")]
        let coordinator = base_config
            .user_input_coordinator
            .clone()
            .unwrap_or_else(|| Arc::new(UserInputCoordinator::new()));

        let launcher = Box::new(InProcessLauncher {
            registry: registry.clone(),
            roci_config: roci_config.clone(),
        });

        let (event_tx, _) = broadcast::channel(256);
        let semaphore = Arc::new(Semaphore::new(supervisor_config.max_concurrent));

        Self {
            registry,
            roci_config,
            config: supervisor_config,
            profile_registry,
            prompt_policy: SubagentPromptPolicy::default(),
            base_config,
            launcher,
            #[cfg(feature = "agent")]
            coordinator,
            event_tx,
            children: Arc::new(Mutex::new(HashMap::new())),
            concurrency_semaphore: semaphore,
        }
    }

    /// Spawn a new child sub-agent from a [`SubagentSpec`].
    ///
    /// Convenience wrapper that uses an empty [`SubagentContext`].
    /// Use [`spawn_with_context`](Self::spawn_with_context) to pass
    /// materialized parent context (summary, snapshot, etc.).
    pub async fn spawn(&self, spec: SubagentSpec) -> Result<SubagentHandle, RociError> {
        self.spawn_with_context(spec, SubagentContext::default())
            .await
    }

    /// Spawn a new child sub-agent from a [`SubagentSpec`] and pre-materialized
    /// [`SubagentContext`].
    ///
    /// The full initial message list (system prompt, context, task/continuation)
    /// is built from the spec and context, then seeded into the child runtime.
    /// The child is started via `continue_without_input()` so the composed
    /// prompt policy is applied exactly once — as the first message.
    ///
    /// Returns a [`SubagentHandle`] immediately. The child runs in a
    /// background tokio task and is subject to the concurrency semaphore.
    pub async fn spawn_with_context(
        &self,
        spec: SubagentSpec,
        context: SubagentContext,
    ) -> Result<SubagentHandle, RociError> {
        // 1. Check max_active_children hard cap
        if let Some(max) = self.config.max_active_children {
            let children = self.children.lock().await;
            let active = children
                .values()
                .filter(|c| {
                    c.status
                        .try_lock()
                        .map(|s| matches!(*s, SubagentStatus::Pending | SubagentStatus::Running))
                        .unwrap_or(true)
                })
                .count();
            if active >= max {
                return Err(RociError::Configuration(format!(
                    "max active children ({max}) reached"
                )));
            }
        }

        // 2. Resolve profile + model
        let profile = self
            .profile_registry
            .resolve_effective(&spec.profile, &spec.overrides)?;
        let model =
            self.profile_registry
                .resolve_model(&profile, &self.registry, &self.roci_config)?;

        // 3. Build initial messages (system prompt + context + task/continuation).
        //    The composed prompt policy is the first (system) message.
        let initial_messages = build_child_initial_messages(
            &spec.input,
            &context,
            &self.prompt_policy,
            &profile,
            &spec.overrides,
        );

        // 4. Generate ID
        let id: SubagentId = Uuid::new_v4();

        // 5. Build child event sink wrapping events as SubagentEvent::AgentEvent
        let child_event_sink =
            super::events::build_child_event_sink(id, spec.label.clone(), self.event_tx.clone());

        // 6. Launch child runtime seeded with the full initial messages.
        //    System prompt is in the messages, not in the runtime config.
        let launched = self
            .launcher
            .launch(
                id,
                model.clone(),
                initial_messages,
                self.base_config.tools.clone(),
                #[cfg(feature = "agent")]
                self.coordinator.clone(),
                Some(child_event_sink),
            )
            .await?;

        // 7. Shared status
        let status = Arc::new(Mutex::new(SubagentStatus::Pending));

        // 8. Snapshot watch channel
        let initial_snapshot = SubagentSnapshot {
            subagent_id: id,
            profile: spec.profile.clone(),
            label: spec.label.clone(),
            model: Some(model.clone()),
            status: SubagentStatus::Pending,
            turn_index: 0,
            message_count: 0,
            is_streaming: false,
            last_error: None,
        };
        let (snapshot_tx, snapshot_rx) = watch::channel(initial_snapshot);

        // 9. Create shared cancellation token
        let cancel_token = CancellationToken::new();

        // 10. Register child entry
        {
            let entry = ChildEntry {
                id,
                label: spec.label.clone(),
                profile: spec.profile.clone(),
                model: Some(model.clone()),
                status: status.clone(),
                cancel_token: cancel_token.clone(),
            };
            self.children.lock().await.insert(id, entry);
        }

        // 11. Emit spawned event
        let _ = self.event_tx.send(SubagentEvent::Spawned {
            subagent_id: id,
            label: spec.label.clone(),
            profile: spec.profile.clone(),
            model: Some(model.clone()),
        });

        // 12. Channels for handle <-> task communication
        let (completion_tx, completion_rx) = oneshot::channel::<SubagentRunResult>();

        // 13. Spawn background task
        {
            let semaphore = self.concurrency_semaphore.clone();
            let event_tx = self.event_tx.clone();
            let status_clone = status.clone();
            let snapshot_tx = snapshot_tx;
            let child_id = id;
            let profile_name = spec.profile.clone();
            let label = spec.label.clone();
            let model_clone = model.clone();
            let runtime = launched.runtime;
            let task_cancel_token = cancel_token.clone();

            tokio::spawn(async move {
                // Acquire semaphore permit (blocks if at capacity)
                let _permit = semaphore.acquire().await;

                // Transition to Running
                {
                    let mut s = status_clone.lock().await;
                    *s = SubagentStatus::Running;
                }
                let _ = event_tx.send(SubagentEvent::StatusChanged {
                    subagent_id: child_id,
                    status: SubagentStatus::Running,
                });
                let _ = snapshot_tx.send(SubagentSnapshot {
                    subagent_id: child_id,
                    profile: profile_name.clone(),
                    label: label.clone(),
                    model: Some(model_clone.clone()),
                    status: SubagentStatus::Running,
                    turn_index: 0,
                    message_count: 0,
                    is_streaming: true,
                    last_error: None,
                });

                // Run child from seeded messages via continue_without_input(),
                // racing against cancellation. Keep the run future alive across
                // the cancel path so abort can unwind the active run to a real
                // terminal result instead of dropping the in-flight future.
                let run_future = runtime.continue_without_input();
                tokio::pin!(run_future);
                let run_result = tokio::select! {
                    result = &mut run_future => result,
                    _ = task_cancel_token.cancelled() => {
                        runtime.abort().await;
                        run_future.await
                    }
                };

                // Map to SubagentRunResult
                let (final_status, subagent_result) = match run_result {
                    Ok(rr) => {
                        let st = match rr.status {
                            RunStatus::Completed => SubagentStatus::Completed,
                            RunStatus::Failed => SubagentStatus::Failed,
                            RunStatus::Canceled => SubagentStatus::Aborted,
                            RunStatus::Running => SubagentStatus::Running,
                        };
                        let result = SubagentRunResult {
                            subagent_id: child_id,
                            status: st,
                            messages: rr.messages,
                            error: rr.error,
                        };
                        (st, result)
                    }
                    Err(e) => {
                        let err_msg = e.to_string();
                        let st = if err_msg.contains("aborted") {
                            SubagentStatus::Aborted
                        } else {
                            SubagentStatus::Failed
                        };
                        let result = SubagentRunResult {
                            subagent_id: child_id,
                            status: st,
                            messages: Vec::new(),
                            error: Some(err_msg),
                        };
                        (st, result)
                    }
                };

                // Update shared status
                {
                    let mut s = status_clone.lock().await;
                    *s = final_status;
                }

                // Emit terminal event
                match final_status {
                    SubagentStatus::Completed => {
                        let _ = event_tx.send(SubagentEvent::Completed {
                            subagent_id: child_id,
                            result: subagent_result.clone(),
                        });
                    }
                    SubagentStatus::Failed => {
                        let _ = event_tx.send(SubagentEvent::Failed {
                            subagent_id: child_id,
                            error: subagent_result
                                .error
                                .clone()
                                .unwrap_or_else(|| "unknown".into()),
                        });
                    }
                    SubagentStatus::Aborted => {
                        let _ = event_tx.send(SubagentEvent::Aborted {
                            subagent_id: child_id,
                        });
                    }
                    _ => {}
                }

                // Final snapshot
                let _ = snapshot_tx.send(SubagentSnapshot {
                    subagent_id: child_id,
                    profile: profile_name,
                    label,
                    model: Some(model_clone),
                    status: final_status,
                    turn_index: 0,
                    message_count: subagent_result.messages.len(),
                    is_streaming: false,
                    last_error: subagent_result.error.clone(),
                });

                // Send completion to handle
                let _ = completion_tx.send(subagent_result);
            });
        }

        // 14. Build and return handle
        let handle = SubagentHandle::new(
            id,
            spec.label,
            spec.profile,
            Some(model),
            status,
            snapshot_rx,
            cancel_token,
            completion_rx,
        );

        Ok(handle)
    }

    /// Abort a specific child by ID.
    ///
    /// Returns `true` if the child was found and a cancellation signal was sent.
    pub async fn abort(&self, id: SubagentId) -> Result<bool, RociError> {
        let children = self.children.lock().await;
        let entry = children
            .get(&id)
            .ok_or_else(|| RociError::Configuration(format!("subagent {id} not found")))?;
        let status = *entry.status.lock().await;
        if status != SubagentStatus::Running && status != SubagentStatus::Pending {
            return Ok(false);
        }
        if entry.cancel_token.is_cancelled() {
            return Ok(false);
        }
        entry.cancel_token.cancel();
        Ok(true)
    }

    /// Wait for a specific child to complete.
    ///
    /// Returns the child's run result. If the child already reached a terminal
    /// state before this call subscribed, or if the event receiver lagged and
    /// the result must be reconstructed from cached status, the returned result
    /// is status-only and may have `messages: Vec::new()` with no structured
    /// error payload. Returns an error if the child ID is unknown.
    pub async fn wait(&self, id: SubagentId) -> Result<SubagentRunResult, RociError> {
        // Check the child exists and whether it's already finished.
        {
            let children = self.children.lock().await;
            let entry = children
                .get(&id)
                .ok_or_else(|| RociError::Configuration(format!("subagent {id} not found")))?;
            let status = *entry.status.lock().await;
            if is_terminal(status) {
                return Ok(SubagentRunResult {
                    subagent_id: id,
                    status,
                    messages: Vec::new(),
                    error: None,
                });
            }
        }

        // Subscribe and wait for a terminal event for this child.
        let mut rx = self.event_tx.subscribe();
        loop {
            match rx.recv().await {
                Ok(SubagentEvent::Completed {
                    subagent_id,
                    result,
                }) if subagent_id == id => return Ok(result),
                Ok(SubagentEvent::Failed { subagent_id, error }) if subagent_id == id => {
                    return Ok(SubagentRunResult {
                        subagent_id: id,
                        status: SubagentStatus::Failed,
                        messages: Vec::new(),
                        error: Some(error),
                    });
                }
                Ok(SubagentEvent::Aborted { subagent_id }) if subagent_id == id => {
                    return Ok(SubagentRunResult {
                        subagent_id: id,
                        status: SubagentStatus::Aborted,
                        messages: Vec::new(),
                        error: None,
                    });
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(RociError::InvalidState(
                        "event channel closed while waiting".into(),
                    ));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Missed some events; check status directly.
                    let children = self.children.lock().await;
                    if let Some(entry) = children.get(&id) {
                        let status = *entry.status.lock().await;
                        if is_terminal(status) {
                            return Ok(SubagentRunResult {
                                subagent_id: id,
                                status,
                                messages: Vec::new(),
                                error: None,
                            });
                        }
                    }
                    // Continue listening
                }
                _ => {
                    // Not our event, keep waiting
                }
            }
        }
    }

    /// Wait for the next child to complete (any child).
    ///
    /// Returns the next observed completion for an active child. If the
    /// completion event was missed and terminal state is reconstructed from the
    /// cached child status, the embedded result is status-only and may have
    /// `messages: Vec::new()` with no structured error payload. Returns `None`
    /// if there are no active children.
    pub async fn wait_any(&self) -> Option<SubagentCompletion> {
        // Collect active child IDs
        let active_ids: Vec<SubagentId> = {
            let children = self.children.lock().await;
            let mut ids = Vec::new();
            for entry in children.values() {
                let status = *entry.status.lock().await;
                if !is_terminal(status) {
                    ids.push(entry.id);
                }
            }
            ids
        };

        if active_ids.is_empty() {
            return None;
        }

        let mut rx = self.event_tx.subscribe();
        loop {
            match rx.recv().await {
                Ok(SubagentEvent::Completed {
                    subagent_id,
                    result,
                }) if active_ids.contains(&subagent_id) => {
                    let children = self.children.lock().await;
                    let entry = children.get(&subagent_id);
                    return Some(SubagentCompletion {
                        subagent_id,
                        label: entry.and_then(|e| e.label.clone()),
                        profile: entry.map(|e| e.profile.clone()).unwrap_or_default(),
                        result,
                    });
                }
                Ok(SubagentEvent::Failed { subagent_id, error })
                    if active_ids.contains(&subagent_id) =>
                {
                    let children = self.children.lock().await;
                    let entry = children.get(&subagent_id);
                    return Some(SubagentCompletion {
                        subagent_id,
                        label: entry.and_then(|e| e.label.clone()),
                        profile: entry.map(|e| e.profile.clone()).unwrap_or_default(),
                        result: SubagentRunResult {
                            subagent_id,
                            status: SubagentStatus::Failed,
                            messages: Vec::new(),
                            error: Some(error),
                        },
                    });
                }
                Ok(SubagentEvent::Aborted { subagent_id }) if active_ids.contains(&subagent_id) => {
                    let children = self.children.lock().await;
                    let entry = children.get(&subagent_id);
                    return Some(SubagentCompletion {
                        subagent_id,
                        label: entry.and_then(|e| e.label.clone()),
                        profile: entry.map(|e| e.profile.clone()).unwrap_or_default(),
                        result: SubagentRunResult {
                            subagent_id,
                            status: SubagentStatus::Aborted,
                            messages: Vec::new(),
                            error: None,
                        },
                    });
                }
                Err(broadcast::error::RecvError::Closed) => return None,
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Check if any active child finished while we lagged
                    let children = self.children.lock().await;
                    for &id in &active_ids {
                        if let Some(entry) = children.get(&id) {
                            let status = *entry.status.lock().await;
                            if is_terminal(status) {
                                return Some(SubagentCompletion {
                                    subagent_id: id,
                                    label: entry.label.clone(),
                                    profile: entry.profile.clone(),
                                    result: SubagentRunResult {
                                        subagent_id: id,
                                        status,
                                        messages: Vec::new(),
                                        error: None,
                                    },
                                });
                            }
                        }
                    }
                }
                _ => {
                    // Not a terminal event for our children
                }
            }
        }
    }

    /// Wait for all active children to complete.
    ///
    /// Returns a completion record for each child in completion order. Because
    /// this delegates to [`Self::wait_any`], individual results may be
    /// status-only fallbacks when terminal state is reconstructed after the
    /// receiver misses completion events.
    pub async fn wait_all(&self) -> Vec<SubagentCompletion> {
        let mut results = Vec::new();
        while let Some(completion) = self.wait_any().await {
            results.push(completion);
        }
        results
    }

    /// Abort all active children and wait for them to finish.
    pub async fn shutdown(&self) {
        // Cancel all active children
        {
            let children = self.children.lock().await;
            for entry in children.values() {
                let status = *entry.status.lock().await;
                if !is_terminal(status) {
                    entry.cancel_token.cancel();
                }
            }
        }
        // Wait for all to finish
        self.wait_all().await;
    }

    /// List all active (Pending or Running) children.
    pub async fn list_active(&self) -> Vec<SubagentSummary> {
        let children = self.children.lock().await;
        let mut result = Vec::new();
        for entry in children.values() {
            let status = *entry.status.lock().await;
            if matches!(status, SubagentStatus::Pending | SubagentStatus::Running) {
                result.push(SubagentSummary {
                    subagent_id: entry.id,
                    label: entry.label.clone(),
                    profile: entry.profile.clone(),
                    model: entry.model.clone(),
                    status,
                });
            }
        }
        result
    }

    /// Subscribe to sub-agent events.
    pub fn subscribe(&self) -> broadcast::Receiver<SubagentEvent> {
        self.event_tx.subscribe()
    }

    /// Submit a user input response for a child's `ask_user` request.
    ///
    /// Delegates to the shared [`UserInputCoordinator`]. The response is
    /// routed by `request_id` to the correct child.
    #[cfg(feature = "agent")]
    pub async fn submit_user_input(
        &self,
        response: crate::tools::UserInputResponse,
    ) -> Result<(), crate::tools::UnknownUserInputRequest> {
        self.coordinator.submit_response(response).await
    }
}

impl Drop for SubagentSupervisor {
    fn drop(&mut self) {
        if self.config.abort_on_drop {
            if let Ok(children) = self.children.try_lock() {
                for entry in children.values() {
                    entry.cancel_token.cancel();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check whether a status is terminal.
fn is_terminal(status: SubagentStatus) -> bool {
    matches!(
        status,
        SubagentStatus::Completed | SubagentStatus::Failed | SubagentStatus::Aborted
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::runtime::AgentRuntime;
    use crate::agent::subagents::context::materialize_context;
    use crate::agent::subagents::types::{ModelCandidate, SnapshotMode, SubagentInput};
    use crate::agent::subagents::{SubagentProfileRegistry, SubagentSupervisorConfig};
    use crate::error::RociError as TestRociError;
    use crate::provider::factory::ProviderFactory;
    use crate::provider::ModelProvider;
    use crate::types::{ModelMessage, Role};
    use async_trait::async_trait;

    // -----------------------------------------------------------------------
    // Dummy provider factory so model resolution succeeds in tests
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Mock launcher that captures initial_messages for assertions
    // -----------------------------------------------------------------------

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
        ) -> Result<LaunchedChild, RociError> {
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
                }
            };
            let runtime =
                AgentRuntime::new(self.registry.clone(), self.roci_config.clone(), config);
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

    // -----------------------------------------------------------------------
    // Basic construction
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // spawn_with_context: prompt-only mode passes correct messages
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // spawn_with_context: snapshot-only mode succeeds (was broken before)
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // spawn_with_context: prompt+snapshot mode
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // System prompt applied exactly once
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Backward-compat: spawn() delegates to spawn_with_context
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Full read-only snapshot mode preserves user/assistant messages
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn spawn_full_snapshot_preserves_conversation() {
        let (supervisor, captured) = make_supervisor_with_mock();

        let parent_messages = vec![
            ModelMessage::system("parent sys"),
            ModelMessage::user("question"),
            ModelMessage::assistant("answer"),
            ModelMessage::user("follow-up"),
        ];
        let context =
            materialize_context(&parent_messages, &SnapshotMode::FullReadonlySnapshot, None);
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

    // -----------------------------------------------------------------------
    // Existing tests (lifecycle, abort, wait, etc.)
    // -----------------------------------------------------------------------

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
}
