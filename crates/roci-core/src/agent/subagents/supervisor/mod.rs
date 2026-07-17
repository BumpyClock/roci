//! Sub-agent supervisor: lifecycle management, concurrency, and event forwarding.
//!
//! [`SubagentSupervisor`] is the public entry point. It owns:
//! - profile resolution and model-candidate fallback (launch-time, not mid-run)
//! - concurrency via `Semaphore` (`max_concurrent`, default 4)
//! - event forwarding from children to parent via `broadcast::channel`
//! - abort, wait, and shutdown orchestration
//! - shared [`crate::agent::runtime::HumanInteractionCoordinator`] across all children
//!
//! See `docs/ARCHITECTURE.md` §"Sub-Agent Supervisor" for design rationale,
//! named-profile registry, model fallback rules, input modes, and the
//! deferred peer-bus seam.
//!
//! Internal structure:
//! - `child_registry` — child entry bookkeeping and status helpers
//! - `orchestration` — high-level `run_parallel`, `race`, and watch helpers
//! - `run_task` — background task that drives a child to completion
//! - `wait` — wait / drain / shutdown methods

mod child_registry;
mod orchestration;
mod run_task;
mod wait;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, oneshot, watch, Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::runtime::AgentConfig;
#[cfg(feature = "agent")]
use crate::agent::runtime::HumanInteractionCoordinator;
use crate::config::RociConfig;
use crate::error::RociError;
use crate::provider::ProviderRegistry;

use super::context::build_child_initial_messages;
use super::events::{emit_subagent_event, CriticalSubagentEventSink};
use super::handle::SubagentHandle;
use super::launcher::{
    build_child_config, select_child_dynamic_tool_providers, select_child_tools, InProcessLauncher,
    SubagentLauncher,
};
use super::profiles::SubagentProfileRegistry;
use super::prompt::SubagentPromptPolicy;
use super::types::{
    SubagentContext, SubagentEvent, SubagentId, SubagentRunResult, SubagentSnapshot, SubagentSpec,
    SubagentStatus, SubagentSummary, SubagentSupervisorConfig,
};

use child_registry::ChildEntry;

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
    config: SubagentSupervisorConfig,
    registry: Arc<ProviderRegistry>,
    roci_config: RociConfig,
    profile_registry: SubagentProfileRegistry,
    prompt_policy: SubagentPromptPolicy,
    base_config: AgentConfig,
    launcher: Box<dyn SubagentLauncher>,
    #[cfg(feature = "agent")]
    coordinator: Arc<HumanInteractionCoordinator>,
    event_tx: broadcast::Sender<SubagentEvent>,
    critical_event_sink: Option<CriticalSubagentEventSink>,
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
        Self::new_with_critical_event_sink(
            registry,
            roci_config,
            base_config,
            supervisor_config,
            profile_registry,
            None,
        )
    }

    pub(crate) fn new_with_critical_event_sink(
        registry: Arc<ProviderRegistry>,
        roci_config: RociConfig,
        base_config: AgentConfig,
        supervisor_config: SubagentSupervisorConfig,
        profile_registry: SubagentProfileRegistry,
        critical_event_sink: Option<CriticalSubagentEventSink>,
    ) -> Self {
        #[cfg(feature = "agent")]
        let coordinator = base_config
            .human_interaction_coordinator
            .clone()
            .unwrap_or_else(|| Arc::new(HumanInteractionCoordinator::new()));

        let launcher = Box::new(InProcessLauncher {
            registry: registry.clone(),
            roci_config: roci_config.clone(),
        });

        let (event_tx, _) = broadcast::channel(256);
        let semaphore = Arc::new(Semaphore::new(supervisor_config.max_concurrent));

        Self {
            config: supervisor_config,
            registry,
            roci_config,
            profile_registry,
            prompt_policy: SubagentPromptPolicy::default(),
            base_config,
            launcher,
            #[cfg(feature = "agent")]
            coordinator,
            event_tx,
            critical_event_sink,
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
        let (handle, start_tx) = self.spawn_with_context_paused(spec, context).await?;
        let _ = start_tx.send(());
        Ok(handle)
    }

    pub(super) async fn spawn_paused(
        &self,
        spec: SubagentSpec,
    ) -> Result<(SubagentHandle, oneshot::Sender<()>), RociError> {
        self.spawn_with_context_paused(spec, SubagentContext::default())
            .await
    }

    async fn spawn_with_context_paused(
        &self,
        spec: SubagentSpec,
        context: SubagentContext,
    ) -> Result<(SubagentHandle, oneshot::Sender<()>), RociError> {
        if self.config.max_concurrent == 0 {
            return Err(RociError::Configuration(
                "max_concurrent must be greater than zero".into(),
            ));
        }

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
        let resolved_model = self.profile_registry.resolve_model_candidates_with_auth(
            &profile,
            &self.registry,
            &self.roci_config,
            &self.base_config,
        )?;
        let model = resolved_model
            .candidates
            .first()
            .cloned()
            .ok_or_else(|| RociError::Configuration("no model candidates".into()))?;

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
        let child_event_sink = super::events::build_child_event_sink_with_critical_sink(
            id,
            spec.label.clone(),
            self.event_tx.clone(),
            self.critical_event_sink.clone(),
        );

        let child_tools = select_child_tools(&self.base_config.tools, &profile.tools)?;
        let child_dynamic_tool_providers = select_child_dynamic_tool_providers(
            &self.base_config.dynamic_tool_providers,
            &profile.mcp_servers,
        )?;
        let mut child_config = build_child_config(
            &self.base_config,
            resolved_model.candidates.clone(),
            child_tools,
            resolved_model.reasoning_effort.as_deref(),
            Some(child_event_sink),
            #[cfg(feature = "agent")]
            self.coordinator.clone(),
        )?;
        child_config.dynamic_tool_providers = child_dynamic_tool_providers;

        // 6. Launch child runtime seeded with the full initial messages.
        //    System prompt is in the messages, not in the runtime config.
        let launched = self
            .launcher
            .launch(id, initial_messages, child_config)
            .await?;
        let handle_runtime = launched.runtime.clone();

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
                snapshot_rx: snapshot_rx.clone(),
                cancel_token: cancel_token.clone(),
            };
            self.children.lock().await.insert(id, entry);
        }

        // 11. Emit spawned event
        emit_subagent_event(
            &self.event_tx,
            self.critical_event_sink.as_ref(),
            SubagentEvent::Spawned {
                subagent_id: id,
                label: spec.label.clone(),
                profile: spec.profile.clone(),
                model: Some(model.clone()),
            },
        );

        // 12. Channels for handle <-> task communication
        let (completion_tx, completion_rx) = oneshot::channel::<SubagentRunResult>();
        let (start_tx, start_rx) = oneshot::channel();

        // 13. Spawn the child behind a routing-controlled start gate.
        let task_status = status.clone();
        let task_profile = spec.profile.clone();
        let task_label = spec.label.clone();
        let task_model = model.clone();
        let task_cancel_token = cancel_token.clone();
        let task_semaphore = self.concurrency_semaphore.clone();
        let task_event_tx = self.event_tx.clone();
        let task_critical_event_sink = self.critical_event_sink.clone();
        tokio::spawn(async move {
            if start_rx.await.is_err() {
                // Routing never published its canonical Spawned event. Resolve
                // the handle without emitting a later out-of-order event.
                let result = SubagentRunResult {
                    subagent_id: id,
                    status: SubagentStatus::Aborted,
                    messages: Vec::new(),
                    error: Some("subagent start canceled".into()),
                };
                *task_status.lock().await = SubagentStatus::Aborted;
                let _ = snapshot_tx.send(SubagentSnapshot {
                    subagent_id: id,
                    profile: task_profile,
                    label: task_label,
                    model: Some(task_model),
                    status: SubagentStatus::Aborted,
                    turn_index: 0,
                    message_count: 0,
                    is_streaming: false,
                    last_error: result.error.clone(),
                });
                let _ = completion_tx.send(result);
                return;
            }

            run_task::run_child_task(
                task_semaphore,
                task_event_tx,
                task_critical_event_sink,
                task_status,
                snapshot_tx,
                id,
                task_profile,
                task_label,
                task_model,
                launched.runtime,
                task_cancel_token,
                profile.default_timeout_ms,
                completion_tx,
            )
            .await;
        });

        // 14. Build and return handle
        let handle = SubagentHandle::new(
            id,
            spec.label,
            spec.profile,
            Some(model),
            handle_runtime,
            status,
            snapshot_rx,
            cancel_token,
            completion_rx,
        );

        Ok((handle, start_tx))
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
    /// Delegates to the shared [`HumanInteractionCoordinator`]. The response is
    /// routed by `request_id` to the correct child.
    #[cfg(feature = "agent")]
    pub async fn submit_user_input(
        &self,
        response: crate::tools::UserInputResponse,
    ) -> Result<(), crate::tools::UnknownUserInputRequest> {
        self.coordinator.submit_user_input_response(response).await
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
