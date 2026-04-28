use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{watch, MutexGuard};

use super::{
    AgentRuntime, AgentRuntimeError, AgentRuntimeEvent, AgentSnapshot, AgentState, RuntimeCursor,
    RuntimeEventPublishRequest, RuntimeSnapshot, RuntimeSubscription, ThreadId, ThreadSnapshot,
};
use crate::error::RociError;
use crate::types::{ModelMessage, Usage};

impl AgentRuntime {
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

    /// Get a snapshot of the cumulative session usage.
    ///
    /// Returns the accumulated token usage across all completed runs since
    /// the last [`reset()`](Self::reset). Excludes in-flight provider usage
    /// until the current run result is merged back into the ledger.
    ///
    /// This ledger is the source of truth for the `prior_session_*_tokens`
    /// values threaded into each run request.
    pub async fn session_usage(&self) -> Usage {
        self.session_usage.lock().await.clone()
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

    /// Subscribe to semantic runtime events.
    ///
    /// `cursor = None` returns only live events. `Some(cursor)` returns retained
    /// replay events through [`RuntimeSubscription::replay`] and then live events
    /// through [`RuntimeSubscription::recv`].
    pub async fn subscribe(&self, cursor: Option<RuntimeCursor>) -> RuntimeSubscription {
        let live_rx = self.runtime_event_tx.subscribe();
        let replay = if let Some(cursor) = cursor {
            self.runtime_event_store.events_after(cursor).await
        } else {
            Ok(Vec::new())
        };
        RuntimeSubscription::new(replay, live_rx, cursor)
    }

    /// Read the runtime-owned chat projection snapshot.
    pub async fn read_snapshot(&self) -> RuntimeSnapshot {
        self.chat_projector
            .lock()
            .expect("chat projector mutex poisoned")
            .read_snapshot()
    }

    /// Read one runtime-owned chat thread projection snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`AgentRuntimeError::ThreadNotFound`] when `thread_id` is not known.
    pub async fn read_thread(
        &self,
        thread_id: ThreadId,
    ) -> Result<ThreadSnapshot, AgentRuntimeError> {
        self.chat_projector
            .lock()
            .expect("chat projector mutex poisoned")
            .read_thread(thread_id)
    }

    /// Broadcast the current snapshot to all watchers.
    pub(super) async fn broadcast_snapshot(&self) {
        let snapshot = self.snapshot().await;
        let _ = self.snapshot_tx.send(snapshot);
    }

    pub(super) async fn publish_runtime_events(
        &self,
        events: Vec<AgentRuntimeEvent>,
    ) -> Result<(), AgentRuntimeError> {
        self.ensure_runtime_event_publisher().await;
        let mut ack_receivers = Vec::with_capacity(events.len());
        {
            let _send_guard = self.runtime_event_send_lock.lock().map_err(|_| {
                AgentRuntimeError::ProjectionFailed {
                    message: "runtime event send lock poisoned".to_string(),
                }
            })?;
            for event in events {
                let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
                self.runtime_event_publish_tx
                    .send(RuntimeEventPublishRequest {
                        event,
                        ack_tx: Some(ack_tx),
                        error_slot: None,
                    })
                    .map_err(|_| AgentRuntimeError::ProjectionFailed {
                        message: "runtime event publisher closed".to_string(),
                    })?;
                ack_receivers.push(ack_rx);
            }
        }
        for ack_rx in ack_receivers {
            ack_rx
                .await
                .map_err(|_| AgentRuntimeError::ProjectionFailed {
                    message: "runtime event publisher dropped acknowledgement".to_string(),
                })??;
        }
        Ok(())
    }

    pub(super) async fn publish_runtime_event(
        &self,
        event: AgentRuntimeEvent,
    ) -> Result<RuntimeCursor, AgentRuntimeError> {
        self.publish_runtime_event_to(event).await
    }

    pub(super) async fn publish_runtime_event_to(
        &self,
        event: AgentRuntimeEvent,
    ) -> Result<RuntimeCursor, AgentRuntimeError> {
        self.ensure_runtime_event_publisher().await;
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        {
            let _send_guard = self.runtime_event_send_lock.lock().map_err(|_| {
                AgentRuntimeError::ProjectionFailed {
                    message: "runtime event send lock poisoned".to_string(),
                }
            })?;
            self.runtime_event_publish_tx
                .send(RuntimeEventPublishRequest {
                    event,
                    ack_tx: Some(ack_tx),
                    error_slot: None,
                })
                .map_err(|_| AgentRuntimeError::ProjectionFailed {
                    message: "runtime event publisher closed".to_string(),
                })?;
        }
        ack_rx
            .await
            .map_err(|_| AgentRuntimeError::ProjectionFailed {
                message: "runtime event publisher dropped acknowledgement".to_string(),
            })?
    }

    async fn ensure_runtime_event_publisher(&self) {
        let Some(mut publish_rx) = self.runtime_event_publish_rx.lock().await.take() else {
            return;
        };
        let event_store = self.runtime_event_store.clone();
        let event_tx = self.runtime_event_tx.clone();
        tokio::spawn(async move {
            while let Some(request) = publish_rx.recv().await {
                let result = event_store.append(request.event.clone()).await;
                match &result {
                    Ok(_) => {
                        let _ = event_tx.send(request.event);
                    }
                    Err(err) => {
                        if let Some(error_slot) = &request.error_slot {
                            if let Ok(mut stored_error) = error_slot.lock() {
                                if stored_error.is_none() {
                                    *stored_error = Some(err.clone());
                                }
                            }
                        }
                    }
                }
                if let Some(ack_tx) = request.ack_tx {
                    let _ = ack_tx.send(result);
                }
            }
        });
    }

    pub(super) fn queue_runtime_event_to(
        publish_tx: &tokio::sync::mpsc::UnboundedSender<RuntimeEventPublishRequest>,
        send_lock: &StdMutex<()>,
        event: AgentRuntimeEvent,
        error_slot: Arc<StdMutex<Option<AgentRuntimeError>>>,
    ) -> Result<(), AgentRuntimeError> {
        let _send_guard = send_lock
            .lock()
            .map_err(|_| AgentRuntimeError::ProjectionFailed {
                message: "runtime event send lock poisoned".to_string(),
            })?;
        publish_tx
            .send(RuntimeEventPublishRequest {
                event,
                ack_tx: None,
                error_slot: Some(error_slot),
            })
            .map_err(|_| AgentRuntimeError::ProjectionFailed {
                message: "runtime event publisher closed".to_string(),
            })
    }

    /// Atomically transition from Idle → Running.
    ///
    /// Uses a try_lock + immediate check to fail fast without holding the
    /// lock across an await point.
    pub(super) fn transition_to_running(&self) -> Result<(), RociError> {
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

    pub(super) fn lock_state_for_idle_mutation(
        &self,
    ) -> Result<MutexGuard<'_, AgentState>, RociError> {
        let state = self
            .state
            .try_lock()
            .map_err(|_| RociError::InvalidState("Agent is busy (state lock contended)".into()))?;
        if *state != AgentState::Idle {
            return Err(RociError::InvalidState(
                "Agent is not idle; runtime mutation requires idle state".into(),
            ));
        }
        if self.queued_turn_count.lock().map_or(0, |count| *count) > 0 {
            return Err(RociError::InvalidState(
                "Agent has queued turns; runtime mutation requires drained queue".into(),
            ));
        }
        Ok(state)
    }

    pub(super) async fn restore_idle_after_preflight_error(&self) {
        let mut state = self.state.lock().await;
        *state = AgentState::Idle;
        let _ = self.state_tx.send(AgentState::Idle);
        drop(state);
        self.broadcast_snapshot().await;
        self.idle_notify.notify_waiters();
    }

    pub(super) fn map_chat_projection_error(err: AgentRuntimeError) -> RociError {
        RociError::InvalidState(format!("chat projection failed: {err}"))
    }
}
