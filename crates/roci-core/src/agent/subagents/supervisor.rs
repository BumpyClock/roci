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
use crate::types::ModelMessage;

use super::context::build_child_initial_messages;
use super::handle::SubagentHandle;
use super::launcher::{InProcessLauncher, SubagentLauncher};
use super::profiles::SubagentProfileRegistry;
use super::prompt::SubagentPromptPolicy;
use super::types::{
    SubagentCompletion, SubagentContext, SubagentEvent, SubagentId, SubagentRunResult,
    SubagentSnapshot, SubagentSpec, SubagentStatus, SubagentSummary, SubagentSupervisorConfig,
};

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
    /// Returns a [`SubagentHandle`] immediately. The child runs in a
    /// background tokio task and is subject to the concurrency semaphore.
    pub async fn spawn(&self, spec: SubagentSpec) -> Result<SubagentHandle, RociError> {
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

        // 3. Build initial messages (for system prompt composition)
        let context = SubagentContext::default();
        let initial_messages = build_child_initial_messages(
            &spec.input,
            &context,
            &self.prompt_policy,
            &profile,
            &spec.overrides,
        );

        // 4. Extract task prompt (last user message)
        let task = extract_task(&initial_messages);

        // 5. Generate ID
        let id: SubagentId = Uuid::new_v4();

        // 6. Build child event sink wrapping events as SubagentEvent::AgentEvent
        let child_event_sink =
            super::events::build_child_event_sink(id, spec.label.clone(), self.event_tx.clone());

        // 7. Launch child runtime
        let launched = self
            .launcher
            .launch(
                id,
                model.clone(),
                profile.system_prompt.clone(),
                self.base_config.tools.clone(),
                #[cfg(feature = "agent")]
                self.coordinator.clone(),
                Some(child_event_sink),
            )
            .await?;

        // 8. Shared status
        let status = Arc::new(Mutex::new(SubagentStatus::Pending));

        // 9. Snapshot watch channel
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

        // 10. Create shared cancellation token
        let cancel_token = CancellationToken::new();

        // 11. Register child entry
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

        // 12. Emit spawned event
        let _ = self.event_tx.send(SubagentEvent::Spawned {
            subagent_id: id,
            label: spec.label.clone(),
            profile: spec.profile.clone(),
            model: Some(model.clone()),
        });

        // 13. Channels for handle <-> task communication
        let (completion_tx, completion_rx) = oneshot::channel::<SubagentRunResult>();

        // 14. Spawn background task
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

                // Run child: prompt with task, racing against cancellation
                let run_result = if let Some(task) = task {
                    tokio::select! {
                        result = runtime.prompt(task) => result,
                        _ = task_cancel_token.cancelled() => {
                            runtime.abort().await;
                            runtime.wait_for_idle().await;
                            Err(RociError::InvalidState("aborted".into()))
                        }
                    }
                } else {
                    Err(RociError::InvalidState(
                        "no task prompt in SubagentInput".into(),
                    ))
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

        // 15. Build and return handle
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
    /// Returns the child's run result. Returns an error if the child ID is
    /// unknown.
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
    /// Returns `None` if there are no active children.
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
    /// Returns a completion record for each child. Order is completion order.
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

/// Extract the task prompt from the initial message list.
///
/// Looks for the last User-role message.
fn extract_task(messages: &[ModelMessage]) -> Option<String> {
    use crate::types::Role;
    messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(|m| m.text().to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::subagents::{SubagentProfileRegistry, SubagentSupervisorConfig};

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
    fn extract_task_finds_last_user_message() {
        use crate::types::ModelMessage;
        let messages = vec![
            ModelMessage::system("sys prompt"),
            ModelMessage::user("do the thing"),
        ];
        assert_eq!(extract_task(&messages), Some("do the thing".to_string()));
    }

    #[test]
    fn extract_task_returns_none_for_empty() {
        assert_eq!(extract_task(&[]), None);
    }

    #[test]
    fn extract_task_returns_none_for_system_only() {
        let messages = vec![ModelMessage::system("sys")];
        assert_eq!(extract_task(&messages), None);
    }

    #[test]
    fn subscribe_returns_receiver() {
        let supervisor = make_supervisor();
        let _rx = supervisor.subscribe();
    }

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
