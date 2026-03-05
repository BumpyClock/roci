//! Sub-agent handle: external interface for a spawned child.

use std::sync::Arc;

use tokio::sync::{oneshot, watch, Mutex};
use tokio_util::sync::CancellationToken;

use crate::models::LanguageModel;

use super::types::{SubagentId, SubagentRunResult, SubagentSnapshot, SubagentStatus};

// ---------------------------------------------------------------------------
// Handle
// ---------------------------------------------------------------------------

/// Handle to a spawned sub-agent.
///
/// Provides observation (snapshot, status) and control (abort, wait) for a
/// single child. Returned by [`super::supervisor::SubagentSupervisor::spawn`].
pub struct SubagentHandle {
    id: SubagentId,
    label: Option<String>,
    profile_name: String,
    model: Option<LanguageModel>,
    status: Arc<Mutex<SubagentStatus>>,
    snapshot_rx: watch::Receiver<SubagentSnapshot>,
    cancel_token: CancellationToken,
    completion_rx: Mutex<Option<oneshot::Receiver<SubagentRunResult>>>,
}

impl SubagentHandle {
    /// Create a new handle. Called internally by the supervisor.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        id: SubagentId,
        label: Option<String>,
        profile_name: String,
        model: Option<LanguageModel>,
        status: Arc<Mutex<SubagentStatus>>,
        snapshot_rx: watch::Receiver<SubagentSnapshot>,
        cancel_token: CancellationToken,
        completion_rx: oneshot::Receiver<SubagentRunResult>,
    ) -> Self {
        Self {
            id,
            label,
            profile_name,
            model,
            status,
            snapshot_rx,
            cancel_token,
            completion_rx: Mutex::new(Some(completion_rx)),
        }
    }

    /// Unique identifier for this sub-agent instance.
    pub fn id(&self) -> SubagentId {
        self.id
    }

    /// Optional human-readable label.
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// Profile name used to spawn this sub-agent.
    pub fn profile_name(&self) -> &str {
        &self.profile_name
    }

    /// Resolved model, if any.
    pub fn model(&self) -> Option<&LanguageModel> {
        self.model.as_ref()
    }

    /// Subscribe to snapshot changes for this child.
    pub fn watch_snapshot(&self) -> watch::Receiver<SubagentSnapshot> {
        self.snapshot_rx.clone()
    }

    /// Current status of this sub-agent.
    pub async fn status(&self) -> SubagentStatus {
        *self.status.lock().await
    }

    /// Request abort of this sub-agent's run.
    ///
    /// Returns `true` if the abort signal was sent, `false` if the child
    /// was already cancelled or finished.
    pub async fn abort(&self) -> bool {
        if self.cancel_token.is_cancelled() {
            return false;
        }
        self.cancel_token.cancel();
        true
    }

    /// Wait for this sub-agent to complete and return its result.
    ///
    /// The completion receiver is consumed on first call. Subsequent calls
    /// return a failed result.
    pub async fn wait(&self) -> SubagentRunResult {
        let rx = self.completion_rx.lock().await.take();
        if let Some(rx) = rx {
            rx.await.unwrap_or_else(|_| SubagentRunResult {
                subagent_id: self.id,
                status: SubagentStatus::Failed,
                messages: Vec::new(),
                error: Some("completion channel dropped".into()),
            })
        } else {
            SubagentRunResult {
                subagent_id: self.id,
                status: SubagentStatus::Failed,
                messages: Vec::new(),
                error: Some("wait() already consumed".into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_accessors_compile() {
        let id = SubagentId::nil();
        assert_eq!(id, uuid::Uuid::nil());
    }
}
