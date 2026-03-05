//! User input coordinator for blocking `ask_user` capability.
//!
//! Coordinates between tool execution (which requests user input) and
//! the CLI/host (which submits responses). Uses oneshot channels for
//! request/response correlation.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, oneshot, Mutex};

use crate::tools::{
    UnknownUserInputRequest, UserInputError, UserInputRequest, UserInputRequestId,
    UserInputResponse,
};

type UserInputOutcome = Result<UserInputResponse, UserInputError>;
type PendingSender = oneshot::Sender<UserInputOutcome>;
type PendingReceiver = oneshot::Receiver<UserInputOutcome>;
type PendingMap = Arc<Mutex<HashMap<UserInputRequestId, PendingSender>>>;

/// An in-flight user-input request owned by the waiter.
///
/// The pending entry is registered in [`UserInputCoordinator`] and cleaned up
/// deterministically if the wait times out or is canceled before a response
/// arrives.
#[derive(Debug)]
pub struct PendingUserInput {
    coordinator: Arc<UserInputCoordinator>,
    request_id: UserInputRequestId,
    rx: PendingReceiver,
}

/// Coordinates user input requests and responses.
///
/// Tools call [`create_request`] to get an owned pending request handle.
/// The host (CLI/TUI/IDE) calls [`submit_response`] to unblock the waiter.
#[derive(Debug)]
pub struct UserInputCoordinator {
    /// Pending requests waiting for responses.
    pending: PendingMap,
    /// Completion notifications for hosts that need to stop waiting on input.
    completion_tx: broadcast::Sender<UserInputRequestId>,
}

impl Default for UserInputCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl UserInputCoordinator {
    /// Create a new coordinator.
    pub fn new() -> Self {
        let (completion_tx, _) = broadcast::channel(32);
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            completion_tx,
        }
    }

    /// Create a request and return a receiver for the response.
    ///
    /// The caller should await the returned handle to get the response.
    /// If the coordinator receives a response via [`submit_response`],
    /// the pending request will resolve with that response.
    pub async fn create_request(
        &self,
        request: UserInputRequest,
    ) -> Result<PendingUserInput, UserInputError> {
        let (tx, rx) = oneshot::channel();
        let request_id = request.request_id;

        let mut pending = self.pending.lock().await;
        pending.insert(request_id, tx);

        Ok(PendingUserInput {
            coordinator: Arc::new(Self {
                pending: Arc::clone(&self.pending),
                completion_tx: self.completion_tx.clone(),
            }),
            request_id,
            rx,
        })
    }

    /// Submit a response for a pending request.
    ///
    /// Returns an error if the request ID is unknown (already completed,
    /// canceled, or never existed).
    pub async fn submit_response(
        &self,
        response: UserInputResponse,
    ) -> Result<(), UnknownUserInputRequest> {
        let request_id = response.request_id;

        let mut pending = self.pending.lock().await;

        if let Some(tx) = pending.remove(&request_id) {
            // Send the response. If the receiver was dropped, that's fine -
            // the tool execution was likely canceled.
            let _ = tx.send(Ok(response));
            let _ = self.completion_tx.send(request_id);
            Ok(())
        } else {
            Err(UnknownUserInputRequest(request_id))
        }
    }

    /// Submit a typed error for a pending request.
    pub async fn submit_error(
        &self,
        request_id: UserInputRequestId,
        error: UserInputError,
    ) -> Result<(), UnknownUserInputRequest> {
        let mut pending = self.pending.lock().await;

        if let Some(tx) = pending.remove(&request_id) {
            let _ = tx.send(Err(error));
            let _ = self.completion_tx.send(request_id);
            Ok(())
        } else {
            Err(UnknownUserInputRequest(request_id))
        }
    }

    /// Cancel all pending requests.
    ///
    /// Called when the agent is reset or aborted. All pending receivers
    /// will receive a cancellation error.
    pub async fn cancel_all(&self) {
        let mut pending = self.pending.lock().await;
        let pending_ids = pending.keys().copied().collect::<Vec<_>>();
        pending.clear();
        drop(pending);

        for request_id in pending_ids {
            let _ = self.completion_tx.send(request_id);
        }
    }

    /// Return whether a request is still pending.
    pub async fn is_pending(&self, request_id: UserInputRequestId) -> bool {
        let pending = self.pending.lock().await;
        pending.contains_key(&request_id)
    }

    /// Subscribe to request completion notifications.
    pub fn subscribe_completion(&self) -> broadcast::Receiver<UserInputRequestId> {
        self.completion_tx.subscribe()
    }

    async fn remove_request(&self, request_id: UserInputRequestId) {
        let mut pending = self.pending.lock().await;
        let removed = pending.remove(&request_id).is_some();
        drop(pending);
        if removed {
            let _ = self.completion_tx.send(request_id);
        }
    }
}

impl PendingUserInput {
    /// Wait for a user input response with optional timeout.
    pub async fn wait(self, timeout_ms: Option<u64>) -> Result<UserInputResponse, UserInputError> {
        let request_id = self.request_id;
        let coordinator = Arc::clone(&self.coordinator);
        let rx = self.rx;

        let result = if let Some(ms) = timeout_ms {
            match tokio::time::timeout(std::time::Duration::from_millis(ms), rx).await {
                Ok(result) => result,
                Err(_) => {
                    coordinator.remove_request(request_id).await;
                    return Err(UserInputError::Timeout { request_id });
                }
            }
        } else {
            rx.await
        };

        match result {
            Ok(Ok(response)) => {
                if response.canceled {
                    Err(UserInputError::Canceled { request_id })
                } else {
                    Ok(response)
                }
            }
            Ok(Err(error)) => Err(error),
            Err(_) => {
                coordinator.remove_request(request_id).await;
                Err(UserInputError::Canceled { request_id })
            }
        }
    }
}

/// Wait for a user input response with optional timeout.
///
/// This remains as a small adapter for existing call sites and tests.
#[allow(dead_code)]
pub async fn wait_for_response(
    pending: PendingUserInput,
    timeout_ms: Option<u64>,
) -> Result<UserInputResponse, UserInputError> {
    pending.wait(timeout_ms).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn create_and_submit_response() {
        let coordinator = UserInputCoordinator::new();

        let request = UserInputRequest {
            request_id: Uuid::nil(),
            tool_call_id: "call_123".to_string(),
            questions: vec![],
            timeout_ms: None,
        };

        let pending = coordinator.create_request(request.clone()).await.unwrap();

        let response = UserInputResponse {
            request_id: Uuid::nil(),
            answers: vec![],
            canceled: false,
        };

        coordinator.submit_response(response.clone()).await.unwrap();

        let received = pending.wait(None).await.unwrap();
        assert_eq!(received.request_id, Uuid::nil());
        assert!(!received.canceled);
    }

    #[tokio::test]
    async fn submit_unknown_request_returns_error() {
        let coordinator = UserInputCoordinator::new();

        let response = UserInputResponse {
            request_id: Uuid::nil(),
            answers: vec![],
            canceled: false,
        };

        let result = coordinator.submit_response(response).await;
        assert!(result.is_err());
        assert!(matches!(result, Err(UnknownUserInputRequest(_))));
    }

    #[tokio::test]
    async fn submit_error_unblocks_waiter_with_typed_error() {
        let coordinator = UserInputCoordinator::new();
        let request = UserInputRequest {
            request_id: Uuid::new_v4(),
            tool_call_id: "call_error".to_string(),
            questions: vec![],
            timeout_ms: None,
        };

        let pending = coordinator.create_request(request.clone()).await.unwrap();
        coordinator
            .submit_error(
                request.request_id,
                UserInputError::InteractivePromptUnavailable {
                    request_id: request.request_id,
                    reason: "stdin is not a terminal".to_string(),
                },
            )
            .await
            .unwrap();

        let result = pending.wait(None).await;
        assert!(matches!(
            result,
            Err(UserInputError::InteractivePromptUnavailable { .. })
        ));
    }

    #[tokio::test]
    async fn wait_for_response_times_out() {
        let coordinator = UserInputCoordinator::new();
        let request = UserInputRequest {
            request_id: Uuid::nil(),
            tool_call_id: "call_timeout".to_string(),
            questions: vec![],
            timeout_ms: None,
        };
        let pending = coordinator.create_request(request.clone()).await.unwrap();
        // Don't submit a response - let it timeout
        let result = wait_for_response(pending, Some(50)).await;
        assert!(matches!(result, Err(UserInputError::Timeout { .. })));
        assert!(coordinator.pending.lock().await.is_empty());
        let late = coordinator
            .submit_response(UserInputResponse {
                request_id: request.request_id,
                answers: vec![],
                canceled: false,
            })
            .await;
        assert!(matches!(late, Err(UnknownUserInputRequest(_))));
    }

    #[tokio::test]
    async fn cancel_all_drops_pending() {
        let coordinator = UserInputCoordinator::new();

        let request = UserInputRequest {
            request_id: Uuid::nil(),
            tool_call_id: "call_123".to_string(),
            questions: vec![],
            timeout_ms: None,
        };

        let pending = coordinator.create_request(request).await.unwrap();

        coordinator.cancel_all().await;

        let result = pending.wait(None).await;
        assert!(matches!(result, Err(UserInputError::Canceled { .. })));
        assert!(coordinator.pending.lock().await.is_empty());
    }

    #[tokio::test]
    async fn late_submit_after_timeout_returns_unknown_request() {
        let coordinator = UserInputCoordinator::new();
        let request = UserInputRequest {
            request_id: Uuid::new_v4(),
            tool_call_id: "call_late".to_string(),
            questions: vec![],
            timeout_ms: None,
        };

        let pending = coordinator.create_request(request.clone()).await.unwrap();
        let result = wait_for_response(pending, Some(10)).await;
        assert!(matches!(result, Err(UserInputError::Timeout { .. })));

        let late = coordinator
            .submit_response(UserInputResponse {
                request_id: request.request_id,
                answers: vec![],
                canceled: false,
            })
            .await;
        assert!(matches!(late, Err(UnknownUserInputRequest(_))));
    }
}
