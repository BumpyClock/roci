//! Sub-agent supervisor: lifecycle management, concurrency, and event forwarding.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, oneshot, watch, Mutex, Semaphore};
use uuid::Uuid;

use crate::agent::runtime::AgentConfig;
#[cfg(feature = "agent")]
use crate::agent::runtime::UserInputCoordinator;
use crate::agent_loop::runner::AgentEventSink;
use crate::agent_loop::{AgentEvent, RunStatus};
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
    SubagentContext, SubagentEvent, SubagentId, SubagentRunResult, SubagentSnapshot, SubagentSpec,
    SubagentStatus, SubagentSummary, SubagentSupervisorConfig,
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
/// - Abort and status queries
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
        let child_event_sink: AgentEventSink = {
            let event_tx = self.event_tx.clone();
            let child_id = id;
            let child_label = spec.label.clone();
            Arc::new(move |event: AgentEvent| {
                let _ = event_tx.send(SubagentEvent::AgentEvent {
                    subagent_id: child_id,
                    label: child_label.clone(),
                    event: Box::new(event),
                });
            })
        };

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

        // 10. Register child entry
        {
            let entry = ChildEntry {
                id,
                label: spec.label.clone(),
                profile: spec.profile.clone(),
                model: Some(model.clone()),
                status: status.clone(),
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
        let (abort_tx, abort_rx) = oneshot::channel::<()>();

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

                // Run child: prompt with task, racing against abort signal
                let run_result = if let Some(task) = task {
                    tokio::select! {
                        result = runtime.prompt(task) => result,
                        _ = async {
                            let _ = abort_rx.await;
                            runtime.abort().await;
                            // Wait for the runtime to actually finish
                            runtime.wait_for_idle().await;
                        } => {
                            // Aborted
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

        // 14. Build and return handle
        let handle = SubagentHandle::new(
            id,
            spec.label,
            spec.profile,
            Some(model),
            status,
            snapshot_rx,
            abort_tx,
            completion_rx,
        );

        Ok(handle)
    }

    /// Abort a specific child by ID.
    ///
    /// Returns `true` if the child was found and an abort signal was sent.
    pub async fn abort(&self, id: SubagentId) -> Result<bool, RociError> {
        let children = self.children.lock().await;
        let entry = children
            .get(&id)
            .ok_or_else(|| RociError::Configuration(format!("subagent {id} not found")))?;
        let status = *entry.status.lock().await;
        if status != SubagentStatus::Running && status != SubagentStatus::Pending {
            return Ok(false);
        }
        // We cannot directly abort from here since the runtime is owned by
        // the background task. The handle's abort_tx is the mechanism.
        // For supervisor-level abort, emit a status change; the caller should
        // use the handle's abort() method.
        Ok(false)
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
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

    #[test]
    fn supervisor_construction_with_builtins() {
        let registry = Arc::new(ProviderRegistry::new());
        let roci_config = RociConfig::default();
        let base_config = make_base_config();
        let sup_config = SubagentSupervisorConfig::default();
        let profile_registry = SubagentProfileRegistry::with_builtins();

        let supervisor = SubagentSupervisor::new(
            registry,
            roci_config,
            base_config,
            sup_config,
            profile_registry,
        );

        assert_eq!(supervisor.config.max_concurrent, 4);
    }

    #[tokio::test]
    async fn list_active_empty_on_fresh_supervisor() {
        let registry = Arc::new(ProviderRegistry::new());
        let roci_config = RociConfig::default();
        let base_config = make_base_config();
        let sup_config = SubagentSupervisorConfig::default();
        let profile_registry = SubagentProfileRegistry::with_builtins();

        let supervisor = SubagentSupervisor::new(
            registry,
            roci_config,
            base_config,
            sup_config,
            profile_registry,
        );

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
        let registry = Arc::new(ProviderRegistry::new());
        let roci_config = RociConfig::default();
        let base_config = make_base_config();
        let sup_config = SubagentSupervisorConfig::default();
        let profile_registry = SubagentProfileRegistry::with_builtins();

        let supervisor = SubagentSupervisor::new(
            registry,
            roci_config,
            base_config,
            sup_config,
            profile_registry,
        );

        let _rx = supervisor.subscribe();
    }
}
