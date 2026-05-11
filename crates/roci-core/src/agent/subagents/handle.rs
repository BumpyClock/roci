//! Sub-agent handle: external interface for a spawned child.

use std::sync::Arc;

use tokio::sync::{oneshot, watch, Mutex};
use tokio_util::sync::CancellationToken;

use crate::agent::runtime::chat::ThreadId;
use crate::agent::runtime::AgentRuntime;
use crate::attachments::PromptInput;
use crate::error::RociError;
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
    runtime: AgentRuntime,
    status: Arc<Mutex<SubagentStatus>>,
    snapshot_rx: watch::Receiver<SubagentSnapshot>,
    cancel_token: CancellationToken,
    completion_rx: watch::Receiver<Option<SubagentRunResult>>,
}

impl SubagentHandle {
    /// Create a new handle. Called internally by the supervisor.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        id: SubagentId,
        label: Option<String>,
        profile_name: String,
        model: Option<LanguageModel>,
        runtime: AgentRuntime,
        status: Arc<Mutex<SubagentStatus>>,
        snapshot_rx: watch::Receiver<SubagentSnapshot>,
        cancel_token: CancellationToken,
        completion_rx: oneshot::Receiver<SubagentRunResult>,
    ) -> Self {
        let (completion_tx, completion_rx_watch) = watch::channel(None);
        tokio::spawn(async move {
            let result = completion_rx.await.unwrap_or_else(|_| SubagentRunResult {
                subagent_id: id,
                status: SubagentStatus::Failed,
                messages: Vec::new(),
                error: Some("completion channel dropped".into()),
            });
            let _ = completion_tx.send(Some(result));
        });

        Self {
            id,
            label,
            profile_name,
            model,
            runtime,
            status,
            snapshot_rx,
            cancel_token,
            completion_rx: completion_rx_watch,
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

    /// Default runtime thread owned by this child.
    pub fn child_thread_id(&self) -> ThreadId {
        self.runtime.default_thread_id()
    }

    /// Subscribe to snapshot changes for this child.
    pub fn watch_snapshot(&self) -> watch::Receiver<SubagentSnapshot> {
        self.snapshot_rx.clone()
    }

    /// Current status of this sub-agent.
    pub async fn status(&self) -> SubagentStatus {
        *self.status.lock().await
    }

    /// Send a steering message to this child runtime.
    pub async fn send_message(&self, message: impl Into<PromptInput>) -> Result<(), RociError> {
        match self.status().await {
            SubagentStatus::Pending | SubagentStatus::Running => self.runtime.steer(message).await,
            status => Err(RociError::InvalidState(format!(
                "subagent {} is {status:?}; cannot send message",
                self.id
            ))),
        }
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
    /// Completion ownership lives in a background task so dropped wait callers
    /// cannot consume the one-shot result.
    pub async fn wait(&self) -> SubagentRunResult {
        let mut rx = self.completion_rx.clone();
        loop {
            if let Some(result) = rx.borrow().clone() {
                return result;
            }
            if rx.changed().await.is_err() {
                return SubagentRunResult {
                    subagent_id: self.id,
                    status: SubagentStatus::Failed,
                    messages: Vec::new(),
                    error: Some("completion result unavailable".into()),
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::runtime::AgentConfig;
    use crate::config::RociConfig;
    use crate::provider::ProviderRegistry;

    #[test]
    fn handle_accessors_compile() {
        let id = SubagentId::nil();
        assert_eq!(id, uuid::Uuid::nil());
    }

    fn make_handle(status_value: SubagentStatus) -> SubagentHandle {
        make_handle_with_completion(status_value).0
    }

    fn make_handle_with_completion(
        status_value: SubagentStatus,
    ) -> (
        SubagentHandle,
        oneshot::Sender<SubagentRunResult>,
        SubagentId,
    ) {
        let id = SubagentId::nil();
        let status = Arc::new(Mutex::new(status_value));
        let snapshot = SubagentSnapshot {
            subagent_id: id,
            profile: "builtin:developer".into(),
            label: None,
            model: None,
            status: status_value,
            turn_index: 0,
            message_count: 0,
            is_streaming: false,
            last_error: None,
        };
        let (_snapshot_tx, snapshot_rx) = watch::channel(snapshot);
        let (completion_tx, completion_rx) = oneshot::channel();
        let runtime = AgentRuntime::new(
            Arc::new(ProviderRegistry::new()),
            RociConfig::default(),
            AgentConfig::default(),
        );

        (
            SubagentHandle::new(
                id,
                None,
                "builtin:developer".into(),
                None,
                runtime,
                status,
                snapshot_rx,
                CancellationToken::new(),
                completion_rx,
            ),
            completion_tx,
            id,
        )
    }

    #[tokio::test]
    async fn send_message_accepts_active_subagent() {
        let handle = make_handle(SubagentStatus::Running);

        handle
            .send_message("continue with parent note")
            .await
            .expect("active subagent should accept steering message");
    }

    #[tokio::test]
    async fn send_message_rejects_terminal_subagent() {
        let handle = make_handle(SubagentStatus::Completed);

        let err = handle
            .send_message("continue with parent note")
            .await
            .expect_err("terminal subagent should reject steering message");

        assert!(matches!(
            err,
            RociError::InvalidState(message)
                if message.contains("cannot send message")
                    && message.contains("Completed")
        ));
    }

    #[tokio::test]
    async fn wait_is_safe_when_waiter_future_is_dropped() {
        let (handle, completion_tx, id) = make_handle_with_completion(SubagentStatus::Running);

        let timed_out = tokio::time::timeout(std::time::Duration::from_millis(10), handle.wait())
            .await
            .expect_err("first wait should be dropped by timeout");
        assert!(timed_out.to_string().contains("deadline"));

        completion_tx
            .send(SubagentRunResult {
                subagent_id: id,
                status: SubagentStatus::Completed,
                messages: Vec::new(),
                error: None,
            })
            .expect("completion receiver should remain owned by background task");

        let result = handle.wait().await;
        assert_eq!(result.subagent_id, id);
        assert_eq!(result.status, SubagentStatus::Completed);
        assert!(result.error.is_none());
    }
}
