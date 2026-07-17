//! Session-scoped routing controller for sub-agent delegation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{broadcast, mpsc, Mutex};

use crate::agent::runtime::chat::ThreadId;
use crate::agent::runtime::AgentConfig;
use crate::attachments::PromptInput;
use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider::ProviderRegistry;
use crate::types::{ModelMessage, Role};

use super::events::{emit_subagent_event, CriticalSubagentEventSink};
use super::handle::SubagentHandle;
use super::profiles::SubagentProfileRegistry;
use super::supervisor::SubagentSupervisor;
use super::types::{
    DelegateSubagentRequest, DelegateSubagentResult, SendSubagentMessageResult, SubagentCaller,
    SubagentCancelResult, SubagentEvent, SubagentId, SubagentInput, SubagentKnownChild,
    SubagentProfile, SubagentProfileRef, SubagentProfileSummary, SubagentRoutingMetadata,
    SubagentRunResult, SubagentSpec, SubagentStatus, SubagentSupervisorConfig,
};

/// Session-scoped controller for sub-agent management operations.
pub struct SubagentRoutingController {
    supervisor: Arc<SubagentSupervisor>,
    profiles: SubagentProfileRegistry,
    state: Arc<Mutex<RoutingState>>,
    event_tx: broadcast::Sender<SubagentEvent>,
    critical_event_sink: Option<CriticalSubagentEventSink>,
    event_order: Arc<StdMutex<()>>,
    max_depth: u32,
}

#[derive(Default)]
struct RoutingState {
    selected_profile: Option<SubagentProfileRef>,
    children: HashMap<SubagentId, ChildRoutingRecord>,
}

struct ChildRoutingRecord {
    profile: SubagentProfileRef,
    label: Option<String>,
    model: Option<LanguageModel>,
    status: SubagentStatus,
    handle: Option<Arc<SubagentHandle>>,
    cached_result: Option<DelegateSubagentResult>,
    parent_tool_call_id: Option<String>,
    child_thread_id: Option<ThreadId>,
    source_subagent_id: Option<SubagentId>,
    target_subagent_id: Option<SubagentId>,
}

impl SubagentRoutingController {
    /// Create a routing controller and supervisor from one profile registry.
    pub fn new(
        registry: Arc<ProviderRegistry>,
        roci_config: RociConfig,
        base_config: AgentConfig,
        supervisor_config: SubagentSupervisorConfig,
        profiles: SubagentProfileRegistry,
        selected_profile: Option<SubagentProfileRef>,
    ) -> Self {
        Self::new_inner(
            registry,
            roci_config,
            base_config,
            supervisor_config,
            profiles,
            selected_profile,
            None,
        )
    }

    pub(crate) fn new_with_critical_events(
        registry: Arc<ProviderRegistry>,
        roci_config: RociConfig,
        base_config: AgentConfig,
        supervisor_config: SubagentSupervisorConfig,
        profiles: SubagentProfileRegistry,
        selected_profile: Option<SubagentProfileRef>,
    ) -> (Self, mpsc::UnboundedReceiver<SubagentEvent>) {
        // ponytail: queue can grow to process memory ceiling while persistence stalls;
        // switch to bounded backpressure when AgentEventSink supports async delivery.
        let (critical_event_tx, critical_event_rx) = mpsc::unbounded_channel();
        let critical_event_sink: CriticalSubagentEventSink = Arc::new(move |event| {
            let _ = critical_event_tx.send(event);
        });
        (
            Self::new_inner(
                registry,
                roci_config,
                base_config,
                supervisor_config,
                profiles,
                selected_profile,
                Some(critical_event_sink),
            ),
            critical_event_rx,
        )
    }

    fn new_inner(
        registry: Arc<ProviderRegistry>,
        roci_config: RociConfig,
        base_config: AgentConfig,
        supervisor_config: SubagentSupervisorConfig,
        profiles: SubagentProfileRegistry,
        selected_profile: Option<SubagentProfileRef>,
        critical_event_sink: Option<CriticalSubagentEventSink>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let event_order = Arc::new(StdMutex::new(()));
        let forwarded_events = event_tx.clone();
        let forwarded_critical_events = critical_event_sink.clone();
        let forwarded_event_order = event_order.clone();
        let supervisor_event_sink: CriticalSubagentEventSink = Arc::new(move |event| {
            if !matches!(event, SubagentEvent::Spawned { .. }) {
                emit_ordered_subagent_event(
                    &forwarded_event_order,
                    &forwarded_events,
                    forwarded_critical_events.as_ref(),
                    event,
                );
            }
        });
        let supervisor = Arc::new(SubagentSupervisor::new_with_critical_event_sink(
            registry,
            roci_config,
            base_config,
            supervisor_config,
            profiles.clone(),
            Some(supervisor_event_sink),
        ));

        Self {
            supervisor,
            profiles,
            state: Arc::new(Mutex::new(RoutingState {
                selected_profile,
                ..RoutingState::default()
            })),
            event_tx,
            critical_event_sink,
            event_order,
            max_depth: 0,
        }
    }

    /// Configured maximum recursive delegation depth for future recursive routing.
    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }

    /// Subscribe to controller-scoped sub-agent events.
    pub fn subscribe(&self) -> broadcast::Receiver<SubagentEvent> {
        self.event_tx.subscribe()
    }

    /// Read metadata for a known child sub-agent.
    pub async fn metadata(&self, id: SubagentId) -> Option<SubagentRoutingMetadata> {
        let state = self.state.lock().await;
        state
            .children
            .get(&id)
            .map(|record| SubagentRoutingMetadata {
                subagent_id: id,
                profile_id: record.profile.clone(),
                label: record.label.clone(),
                model: record.model.clone(),
                parent_tool_call_id: record.parent_tool_call_id.clone(),
                child_thread_id: record.child_thread_id,
                source_subagent_id: record.source_subagent_id,
                target_subagent_id: record.target_subagent_id,
            })
    }

    /// List available sub-agent profile summaries.
    pub fn list_profiles(
        &self,
        caller: &SubagentCaller,
    ) -> Result<Vec<SubagentProfileSummary>, RociError> {
        authorize_main_agent(caller)?;
        self.profiles.profile_summaries()
    }

    /// Return the selected main/default-agent profile override.
    pub async fn current_profile(
        &self,
        caller: &SubagentCaller,
    ) -> Result<Option<SubagentProfileSummary>, RociError> {
        authorize_main_agent(caller)?;
        let selected = self.state.lock().await.selected_profile.clone();
        selected
            .map(|profile_ref| {
                self.profiles
                    .resolve(&profile_ref)
                    .map(|profile| SubagentProfileSummary::from(&profile))
            })
            .transpose()
    }

    /// Select the profile applied to the main/default agent on future runs.
    pub async fn select_profile(
        &self,
        profile_ref: &str,
        caller: &SubagentCaller,
    ) -> Result<SubagentProfileSummary, RociError> {
        authorize_main_agent(caller)?;
        let profile_ref = self.resolve_profile_ref(Some(profile_ref))?;
        let profile = self.profiles.resolve(&profile_ref)?;
        self.state.lock().await.selected_profile = Some(profile_ref);
        Ok(SubagentProfileSummary::from(&profile))
    }

    /// Clear the selected override. Registry default resolution remains active.
    pub async fn deselect_profile(&self, caller: &SubagentCaller) -> Result<(), RociError> {
        authorize_main_agent(caller)?;
        self.state.lock().await.selected_profile = None;
        Ok(())
    }

    pub(crate) async fn effective_main_profile(
        &self,
        caller: &SubagentCaller,
    ) -> Result<Option<SubagentProfile>, RociError> {
        authorize_main_agent(caller)?;
        let selected = self.state.lock().await.selected_profile.clone();
        selected
            .or_else(|| self.profiles.default_profile_ref())
            .map(|profile_ref| self.profiles.resolve(&profile_ref))
            .transpose()
    }

    /// Delegate a task to a sub-agent.
    pub async fn delegate(
        &self,
        request: DelegateSubagentRequest,
        caller: &SubagentCaller,
    ) -> Result<DelegateSubagentResult, RociError> {
        self.delegate_from_tool(request, caller, None).await
    }

    /// Delegate a task with parent tool-call metadata captured by runtime tools.
    pub async fn delegate_from_tool(
        &self,
        request: DelegateSubagentRequest,
        caller: &SubagentCaller,
        parent_tool_call_id: Option<String>,
    ) -> Result<DelegateSubagentResult, RociError> {
        authorize_main_agent(caller)?;
        let profile = self.resolve_profile_ref(request.profile.as_deref())?;
        let spec = SubagentSpec {
            profile: profile.clone(),
            label: request.label.clone(),
            input: SubagentInput::Prompt { task: request.task },
            overrides: Default::default(),
        };
        let (handle, start_tx) = self.supervisor.spawn_paused(spec).await?;
        let handle = Arc::new(handle);
        let id = handle.id();
        let model = handle.model().cloned();
        let label = handle.label().map(str::to_string);
        let child_thread_id = Some(handle.child_thread_id());

        {
            let mut state = self.state.lock().await;
            state.children.insert(
                id,
                ChildRoutingRecord {
                    profile: profile.clone(),
                    label: label.clone(),
                    model: model.clone(),
                    status: SubagentStatus::Running,
                    handle: Some(handle.clone()),
                    cached_result: None,
                    parent_tool_call_id,
                    child_thread_id,
                    source_subagent_id: None,
                    target_subagent_id: None,
                },
            );
        }
        emit_ordered_subagent_event(
            &self.event_order,
            &self.event_tx,
            self.critical_event_sink.as_ref(),
            SubagentEvent::Spawned {
                subagent_id: id,
                label: label.clone(),
                profile: profile.clone(),
                model: model.clone(),
            },
        );
        let _ = start_tx.send(());

        if request.run_in_background {
            return Ok(running_result(id, profile, child_thread_id));
        }

        let run_result = handle.wait().await;
        let result = compact_result(&profile, &run_result, child_thread_id);
        self.cache_result(id, result.clone()).await;
        Ok(result)
    }

    /// List unwaited child sub-agents known to the routing controller.
    pub async fn list_subagents(
        &self,
        caller: &SubagentCaller,
    ) -> Result<Vec<SubagentKnownChild>, RociError> {
        authorize_main_agent(caller)?;
        let records = {
            let state = self.state.lock().await;
            state
                .children
                .iter()
                .filter(|(_, record)| record.cached_result.is_none())
                .map(|(id, record)| {
                    (
                        *id,
                        record.profile.clone(),
                        record.label.clone(),
                        record.model.clone(),
                        record.status,
                        record.handle.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };

        let mut children = Vec::with_capacity(records.len());
        for (id, profile, label, model, fallback_status, handle) in records {
            let status = if let Some(handle) = handle {
                handle.status().await
            } else {
                fallback_status
            };
            children.push(SubagentKnownChild {
                subagent_id: id,
                profile_id: profile,
                label,
                status,
                model,
            });
        }
        children.sort_by_key(|child| child.subagent_id);
        Ok(children)
    }

    /// Wait for a child to complete and cache its compact result.
    pub async fn wait_subagent(
        &self,
        id: SubagentId,
        caller: &SubagentCaller,
    ) -> Result<DelegateSubagentResult, RociError> {
        authorize_main_agent(caller)?;
        let (profile, handle, child_thread_id) = {
            let state = self.state.lock().await;
            let record = state
                .children
                .get(&id)
                .ok_or_else(|| RociError::Configuration(format!("subagent {id} not found")))?;
            if let Some(result) = &record.cached_result {
                return Ok(result.clone());
            }
            let handle = record.handle.clone().ok_or_else(|| {
                RociError::InvalidState(format!("subagent {id} has no active handle"))
            })?;
            (record.profile.clone(), handle, record.child_thread_id)
        };

        let run_result = handle.wait().await;
        let result = compact_result(&profile, &run_result, child_thread_id);
        self.cache_result(id, result.clone()).await;
        Ok(result)
    }

    /// Cancel an active child sub-agent.
    pub async fn cancel_subagent(
        &self,
        id: SubagentId,
        caller: &SubagentCaller,
    ) -> Result<SubagentCancelResult, RociError> {
        authorize_main_agent(caller)?;
        let (profile, handle, child_thread_id) = {
            let state = self.state.lock().await;
            let record = state
                .children
                .get(&id)
                .ok_or_else(|| RociError::Configuration(format!("subagent {id} not found")))?;
            if let Some(result) = &record.cached_result {
                return Ok(SubagentCancelResult {
                    subagent_id: id,
                    status: result.status,
                    canceled: false,
                });
            }
            let handle = record.handle.clone().ok_or_else(|| {
                RociError::InvalidState(format!("subagent {id} has no active handle"))
            })?;
            (record.profile.clone(), handle, record.child_thread_id)
        };

        let status = handle.status().await;
        if is_terminal(status) {
            let run_result = handle.wait().await;
            let result = compact_result(&profile, &run_result, child_thread_id);
            self.cache_result(id, result.clone()).await;
            return Ok(SubagentCancelResult {
                subagent_id: id,
                status: result.status,
                canceled: false,
            });
        }

        let canceled = handle.abort().await;
        if !canceled {
            let status = handle.status().await;
            if is_terminal(status) {
                let run_result = handle.wait().await;
                let result = compact_result(&profile, &run_result, child_thread_id);
                self.cache_result(id, result.clone()).await;
                return Ok(SubagentCancelResult {
                    subagent_id: id,
                    status: result.status,
                    canceled: false,
                });
            }
            return Ok(SubagentCancelResult {
                subagent_id: id,
                status,
                canceled: false,
            });
        }

        Ok(SubagentCancelResult {
            subagent_id: id,
            status: SubagentStatus::Aborted,
            canceled: true,
        })
    }

    /// Send a steering message to an active child sub-agent.
    pub async fn send_subagent_message(
        &self,
        id: SubagentId,
        message: impl Into<PromptInput>,
        caller: &SubagentCaller,
    ) -> Result<SendSubagentMessageResult, RociError> {
        authorize_main_agent(caller)?;
        let handle = {
            let state = self.state.lock().await;
            let record = state
                .children
                .get(&id)
                .ok_or_else(|| RociError::Configuration(format!("subagent {id} not found")))?;
            if record.cached_result.is_some() || is_terminal(record.status) {
                return Err(RociError::Configuration(format!(
                    "cannot send message to terminal subagent {id}"
                )));
            }
            record.handle.clone().ok_or_else(|| {
                RociError::InvalidState(format!("subagent {id} has no active handle"))
            })?
        };

        let status = handle.status().await;
        if is_terminal(status) {
            return Err(RociError::Configuration(format!(
                "cannot send message to terminal subagent {id}"
            )));
        }
        handle.send_message(message).await?;
        Ok(SendSubagentMessageResult {
            subagent_id: id,
            accepted: true,
        })
    }

    fn resolve_profile_ref(
        &self,
        requested: Option<&str>,
    ) -> Result<SubagentProfileRef, RociError> {
        let profile_ref = match requested {
            Some(profile) => profile.to_string(),
            None => self.profiles.default_profile_ref().ok_or_else(|| {
                RociError::Configuration("no default subagent profile configured".into())
            })?,
        };

        if !self
            .profiles
            .list_profile_refs()
            .iter()
            .any(|profile| profile == &profile_ref)
        {
            return Err(RociError::Configuration(format!(
                "unknown subagent profile '{profile_ref}'"
            )));
        }
        self.profiles.resolve(&profile_ref)?;
        Ok(profile_ref)
    }

    async fn cache_result(&self, id: SubagentId, result: DelegateSubagentResult) {
        let mut state = self.state.lock().await;
        if let Some(record) = state.children.get_mut(&id) {
            record.status = result.status;
            record.cached_result = Some(result);
        }
    }
}

fn emit_ordered_subagent_event(
    event_order: &StdMutex<()>,
    event_tx: &broadcast::Sender<SubagentEvent>,
    critical_event_sink: Option<&CriticalSubagentEventSink>,
    event: SubagentEvent,
) {
    let _guard = event_order
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    emit_subagent_event(event_tx, critical_event_sink, event);
}

fn authorize_main_agent(caller: &SubagentCaller) -> Result<(), RociError> {
    if caller.is_main_agent {
        Ok(())
    } else {
        Err(RociError::Configuration(
            "subagent management tools are only available to the main agent".into(),
        ))
    }
}

fn compact_result(
    profile_id: &SubagentProfileRef,
    result: &SubagentRunResult,
    child_thread_id: Option<ThreadId>,
) -> DelegateSubagentResult {
    compact_result_for_runtime(profile_id, result, child_thread_id)
}

pub(crate) fn compact_result_for_runtime(
    profile_id: &SubagentProfileRef,
    result: &SubagentRunResult,
    child_thread_id: Option<ThreadId>,
) -> DelegateSubagentResult {
    DelegateSubagentResult {
        subagent_id: result.subagent_id,
        profile_id: profile_id.clone(),
        status: result.status,
        summary: summarize_result(result),
        artifacts: Vec::new(),
        child_thread_id,
        usage: None,
        error: result.error.clone(),
    }
}

fn running_result(
    subagent_id: SubagentId,
    profile_id: SubagentProfileRef,
    child_thread_id: Option<ThreadId>,
) -> DelegateSubagentResult {
    DelegateSubagentResult {
        subagent_id,
        profile_id,
        status: SubagentStatus::Running,
        summary: String::new(),
        artifacts: Vec::new(),
        child_thread_id,
        usage: None,
        error: None,
    }
}

fn summarize_result(result: &SubagentRunResult) -> String {
    result
        .messages
        .iter()
        .rev()
        .find(|message| message.role == Role::Assistant)
        .map(ModelMessage::text)
        .unwrap_or_default()
}

fn is_terminal(status: SubagentStatus) -> bool {
    matches!(
        status,
        SubagentStatus::Completed | SubagentStatus::Failed | SubagentStatus::Aborted
    )
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod routing_tests;
