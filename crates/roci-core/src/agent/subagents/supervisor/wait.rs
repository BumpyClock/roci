//! Wait, drain, and shutdown logic for [`SubagentSupervisor`].

use tokio::sync::broadcast;

use crate::agent::subagents::types::{
    SubagentCompletion, SubagentEvent, SubagentId, SubagentRunResult, SubagentStatus,
};
use crate::error::RociError;

use super::child_registry::is_terminal;
use super::SubagentSupervisor;

impl SubagentSupervisor {
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
}
