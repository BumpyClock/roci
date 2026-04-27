use super::{
    AgentRuntime, AgentRuntimeError, AgentRuntimeEventPayload, AgentState, TurnId, TurnSnapshot,
    TurnStatus,
};
use crate::agent_loop::RunResult;
use crate::error::RociError;
use crate::types::{ModelMessage, Role, Usage};

impl AgentRuntime {
    async fn queue_chat_turn(&self, messages: Vec<ModelMessage>) -> Result<TurnId, RociError> {
        let projection = self
            .chat_projector
            .lock()
            .map_err(|_| RociError::InvalidState("chat projector lock poisoned".into()))?
            .queue_turn(messages);
        let turn_id = projection.turn_id;
        self.publish_runtime_events(projection.events)
            .await
            .map_err(Self::map_chat_projection_error)?;
        Ok(turn_id)
    }

    /// Start a new conversation with a user prompt.
    ///
    /// If the message history is empty and a system prompt is configured,
    /// the system prompt is prepended automatically.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the agent is not idle.
    pub async fn prompt(&self, text: impl Into<String>) -> Result<RunResult, RociError> {
        self.transition_to_running()?;

        let text = text.into();
        let system_prompt = self.system_prompt.lock().await.clone();
        let mut msgs = self.messages.lock().await;
        let mut turn_messages = Vec::new();
        if let Some(ref sys) = system_prompt {
            if msgs.is_empty() {
                let system_message = ModelMessage::system(sys.clone());
                msgs.push(system_message.clone());
                turn_messages.push(system_message);
            }
        }
        let user_message = ModelMessage::user(text);
        msgs.push(user_message.clone());
        turn_messages.push(user_message);
        let snapshot = msgs.clone();
        drop(msgs);

        let turn_id = match self.queue_chat_turn(turn_messages).await {
            Ok(turn_id) => turn_id,
            Err(err) => {
                self.restore_idle_after_preflight_error().await;
                return Err(err);
            }
        };
        self.run_loop(snapshot, turn_id).await
    }

    /// Continue the conversation with additional user input.
    ///
    /// Unlike [`prompt`](Self::prompt), this never prepends the system prompt
    /// (it was already added on the first turn).
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the agent is not idle.
    pub async fn continue_run(&self, text: impl Into<String>) -> Result<RunResult, RociError> {
        self.transition_to_running()?;

        let text = text.into();
        let mut msgs = self.messages.lock().await;
        let user_message = ModelMessage::user(text);
        msgs.push(user_message.clone());
        let snapshot = msgs.clone();
        drop(msgs);

        let turn_id = match self.queue_chat_turn(vec![user_message]).await {
            Ok(turn_id) => turn_id,
            Err(err) => {
                self.restore_idle_after_preflight_error().await;
                return Err(err);
            }
        };
        self.run_loop(snapshot, turn_id).await
    }

    /// Continue the conversation without appending a new user message.
    ///
    /// This mirrors pi-mono's `continue()` behavior and is useful for retrying
    /// from existing context or draining queued steering/follow-up messages.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if:
    /// - the agent is not idle,
    /// - there is no message history to continue from,
    /// - the last message is assistant and there are no queued steering/follow-ups.
    pub async fn continue_without_input(&self) -> Result<RunResult, RociError> {
        self.transition_to_running()?;

        let snapshot = self.messages.lock().await.clone();
        if snapshot.is_empty() {
            self.restore_idle_after_preflight_error().await;
            return Err(RociError::InvalidState(
                "No messages to continue from".into(),
            ));
        }

        if matches!(snapshot.last().map(|m| m.role), Some(Role::Assistant)) {
            let has_steering = !self.steering_queue.lock().await.is_empty();
            let has_follow_ups = !self.follow_up_queue.lock().await.is_empty();
            if !has_steering && !has_follow_ups {
                self.restore_idle_after_preflight_error().await;
                return Err(RociError::InvalidState(
                    "Cannot continue from message role: assistant".into(),
                ));
            }
        }

        let turn_id = match self.queue_chat_turn(Vec::new()).await {
            Ok(turn_id) => turn_id,
            Err(err) => {
                self.restore_idle_after_preflight_error().await;
                return Err(err);
            }
        };
        self.run_loop(snapshot, turn_id).await
    }

    /// Queue a steering message to interrupt the current tool execution.
    ///
    /// The message is injected between tool batches on the next iteration.
    /// Does nothing if the agent is idle (the message is still queued and
    /// will be picked up on the next run).
    pub async fn steer(&self, text: impl Into<String>) {
        self.steering_queue
            .lock()
            .await
            .push(ModelMessage::user(text));
    }

    /// Queue a follow-up message to continue after natural completion.
    ///
    /// Follow-up messages are checked when the inner loop ends (no more
    /// tool calls). If present, they extend the conversation.
    pub async fn follow_up(&self, text: impl Into<String>) {
        self.follow_up_queue
            .lock()
            .await
            .push(ModelMessage::user(text));
    }

    /// Abort the current run.
    ///
    /// Returns `true` if an abort signal was successfully sent, `false` if
    /// the agent was not running or the handle was already consumed.
    pub async fn abort(&self) -> bool {
        let active_turn_id = self.chat_projector.lock().ok().and_then(|projector| {
            projector
                .read_thread(projector.default_thread_id())
                .ok()
                .and_then(|thread| thread.active_turn_id)
        });

        if let Some(turn_id) = active_turn_id {
            if self.cancel_turn(turn_id).await.is_ok() {
                return true;
            }
        }

        self.abort_legacy().await
    }

    /// Cancel a semantic chat turn and abort the active provider call when present.
    ///
    /// # Errors
    ///
    /// Returns [`AgentRuntimeError::AlreadyTerminal`] for completed, failed, or
    /// canceled turns. Returns [`AgentRuntimeError::StaleRuntime`] when the
    /// turn id revision no longer matches the current thread revision.
    pub async fn cancel_turn(&self, turn_id: TurnId) -> Result<TurnSnapshot, AgentRuntimeError> {
        let (previous_status, event, canceled) = {
            let mut projector =
                self.chat_projector
                    .lock()
                    .map_err(|_| AgentRuntimeError::ProjectionFailed {
                        message: "chat projector lock poisoned".into(),
                    })?;
            let previous = projector.turn_snapshot(turn_id)?;
            let event = projector.cancel_turn(turn_id)?;
            let canceled = match &event.payload {
                AgentRuntimeEventPayload::TurnCanceled { turn } => turn.clone(),
                _ => {
                    return Err(AgentRuntimeError::ProjectionFailed {
                        message: format!("cancel projection emitted non-cancel event: {turn_id}"),
                    });
                }
            };
            (previous.status, event, canceled)
        };

        let abort_sent = self.abort_active_provider_call().await;
        if abort_sent || previous_status == TurnStatus::Running {
            self.transition_running_to_aborting().await;
        }

        self.publish_runtime_event(event).await?;

        Ok(canceled)
    }

    async fn abort_legacy(&self) -> bool {
        let mut state = self.state.lock().await;
        if *state != AgentState::Running {
            return false;
        }
        *state = AgentState::Aborting;
        let _ = self.state_tx.send(AgentState::Aborting);
        drop(state);
        self.broadcast_snapshot().await;

        let mut abort_tx = self.active_abort_tx.lock().await;
        if let Some(tx) = abort_tx.take() {
            tx.send(()).is_ok()
        } else {
            false
        }
    }

    pub(super) async fn abort_active_provider_call(&self) -> bool {
        let mut abort_tx = self.active_abort_tx.lock().await;
        abort_tx.take().is_some_and(|tx| tx.send(()).is_ok())
    }

    async fn transition_running_to_aborting(&self) {
        let mut state = self.state.lock().await;
        if *state == AgentState::Running {
            *state = AgentState::Aborting;
            let _ = self.state_tx.send(AgentState::Aborting);
        }
        drop(state);
        self.broadcast_snapshot().await;
    }

    /// Reset the agent: abort any in-flight run, then clear messages and queues.
    pub async fn reset(&self) {
        self.abort().await;
        self.wait_for_idle().await;

        #[cfg(feature = "agent")]
        self.user_input_coordinator.cancel_all().await;

        self.messages.lock().await.clear();
        self.steering_queue.lock().await.clear();
        self.follow_up_queue.lock().await.clear();
        *self.turn_index.lock().await = 0;
        *self.is_streaming.lock().await = false;
        *self.last_error.lock().await = None;
        *self.session_usage.lock().await = Usage::default();
        let snapshot = self
            .chat_projector
            .lock()
            .expect("chat projector mutex poisoned")
            .bootstrap_thread(Vec::new())
            .expect("empty chat bootstrap cannot fail");
        if let Err(err) = self
            .runtime_event_store
            .invalidate_thread(snapshot.thread_id, snapshot.last_seq)
            .await
        {
            *self.last_error.lock().await = Some(err.to_string());
        }

        let mut state = self.state.lock().await;
        *state = AgentState::Idle;
        let _ = self.state_tx.send(AgentState::Idle);
        drop(state);
        self.broadcast_snapshot().await;
    }

    /// Wait until the agent is idle.
    ///
    /// Returns immediately if already idle; otherwise blocks until the
    /// current run completes, fails, or is aborted.
    pub async fn wait_for_idle(&self) {
        loop {
            if *self.state.lock().await == AgentState::Idle {
                return;
            }
            self.idle_notify.notified().await;
        }
    }
}
