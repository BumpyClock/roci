use super::{
    AgentRuntime, AgentRuntimeError, AgentRuntimeEventPayload, AgentState, EnqueueTurnRequest,
    ImportedThread, QueuedTurn, ThreadId, TurnId, TurnSnapshot, TurnStatus,
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

    /// Return the runtime's default semantic thread id.
    pub fn default_thread_id(&self) -> ThreadId {
        self.chat_projector
            .lock()
            .expect("chat projector mutex poisoned")
            .default_thread_id()
    }

    /// Import a full semantic thread snapshot and separate provider ledger.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn import_thread(&self, imported: ImportedThread) -> Result<(), RociError> {
        let state_guard = self.lock_state_for_idle_mutation()?;
        let mut existing_messages = self.messages.try_lock().map_err(|_| {
            RociError::InvalidState("Agent is busy (messages lock contended)".into())
        })?;
        let snapshot = self
            .chat_projector
            .lock()
            .map_err(|_| RociError::InvalidState("chat projector lock poisoned".into()))?
            .import_thread(imported.thread)
            .map_err(Self::map_chat_projection_error)?;
        self.runtime_event_store
            .invalidate_thread(snapshot.thread_id, snapshot.last_seq)
            .await
            .map_err(Self::map_chat_projection_error)?;
        *existing_messages = imported.model_messages;
        drop(existing_messages);
        drop(state_guard);
        self.broadcast_snapshot().await;
        Ok(())
    }

    /// Queue a typed chat turn and return its stable id before provider execution.
    ///
    /// Queued turns execute one at a time. Per-turn settings and approval policy
    /// are frozen when this method is called.
    pub async fn enqueue_turn(&self, request: EnqueueTurnRequest) -> Result<TurnId, RociError> {
        let options = self
            .current_turn_options(
                request.generation_settings.clone(),
                request.approval_policy,
                request.collaboration_mode,
            )
            .await;
        self.increment_queued_turn_count();
        let turn_id = match self.queue_chat_turn(request.messages.clone()).await {
            Ok(turn_id) => turn_id,
            Err(err) => {
                self.decrement_queued_turn_count();
                return Err(err);
            }
        };
        let should_spawn = {
            let mut state = self.queued_turn_state.lock().await;
            state.turns.push_back(QueuedTurn {
                turn_id,
                messages: request.messages,
                options,
            });
            if state.worker_active {
                false
            } else {
                state.worker_active = true;
                true
            }
        };
        if should_spawn {
            let agent = self.clone();
            tokio::spawn(async move {
                agent.run_queued_turn_worker().await;
            });
        }
        Ok(turn_id)
    }

    async fn run_queued_turn_worker(self) {
        loop {
            let queued = {
                let mut state = self.queued_turn_state.lock().await;
                let Some(queued) = state.turns.pop_front() else {
                    state.worker_active = false;
                    return;
                };
                queued
            };
            self.run_queued_turn(queued).await;
            self.decrement_queued_turn_count();
        }
    }

    async fn run_queued_turn(&self, queued: QueuedTurn) {
        match self.chat_turn_status(queued.turn_id) {
            Ok(TurnStatus::Canceled) | Err(_) => return,
            Ok(_) => {}
        }

        loop {
            match self.transition_to_running() {
                Ok(()) => break,
                Err(RociError::InvalidState(_)) => {
                    if matches!(
                        self.chat_turn_status(queued.turn_id),
                        Ok(TurnStatus::Canceled) | Err(_)
                    ) {
                        return;
                    }
                    self.wait_for_current_run_idle().await;
                }
                Err(err) => {
                    self.record_background_error(err.to_string()).await;
                    return;
                }
            }
        }

        if matches!(
            self.chat_turn_status(queued.turn_id),
            Ok(TurnStatus::Canceled) | Err(_)
        ) {
            self.restore_idle_after_preflight_error().await;
            return;
        }

        let snapshot = {
            let messages = self.messages.lock().await;
            let mut snapshot = messages.clone();
            snapshot.extend(queued.messages);
            snapshot
        };
        if let Err(err) = self
            .run_loop(snapshot, queued.turn_id, queued.options)
            .await
        {
            self.record_background_error(err.to_string()).await;
        }
    }

    fn increment_queued_turn_count(&self) {
        let mut count = self
            .queued_turn_count
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *count += 1;
    }

    fn decrement_queued_turn_count(&self) {
        let mut count = self
            .queued_turn_count
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *count = count.saturating_sub(1);
        self.queued_turn_notify.notify_waiters();
    }

    fn queued_turn_count_for_wait(&self) -> usize {
        self.queued_turn_count
            .lock()
            .map_or_else(|poisoned| *poisoned.into_inner(), |count| *count)
    }

    async fn wait_for_queued_turns_to_drain(&self) {
        loop {
            let notified = self.queued_turn_notify.notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable();

            if self.queued_turn_count_for_wait() == 0 {
                return;
            }
            notified.as_mut().await;
        }
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
        let options = self.current_turn_options(None, None, None).await;
        self.run_loop(snapshot, turn_id, options).await
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
        let options = self.current_turn_options(None, None, None).await;
        self.run_loop(snapshot, turn_id, options).await
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
        let options = self.current_turn_options(None, None, None).await;
        self.run_loop(snapshot, turn_id, options).await
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
        let (previous_status, events, canceled) = {
            let mut projector =
                self.chat_projector
                    .lock()
                    .map_err(|_| AgentRuntimeError::ProjectionFailed {
                        message: "chat projector lock poisoned".into(),
                    })?;
            let previous = projector.turn_snapshot(turn_id)?;
            let mut events = projector.cancel_pending_approvals(turn_id)?;
            let event = projector.cancel_turn(turn_id)?;
            let canceled = match &event.payload {
                AgentRuntimeEventPayload::TurnCanceled { turn } => turn.clone(),
                _ => {
                    return Err(AgentRuntimeError::ProjectionFailed {
                        message: format!("cancel projection emitted non-cancel event: {turn_id}"),
                    });
                }
            };
            events.push(event);
            (previous.status, events, canceled)
        };

        if previous_status == TurnStatus::Running {
            let abort_sent = self.abort_active_provider_call().await;
            if abort_sent {
                self.transition_running_to_aborting().await;
            }
        }

        self.publish_runtime_events(events).await?;

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

    pub(super) async fn record_background_error(&self, message: String) {
        *self.last_error.lock().await = Some(message);
        self.broadcast_snapshot().await;
    }

    async fn wait_for_current_run_idle(&self) {
        loop {
            let notified = self.idle_notify.notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable();

            if *self.state.lock().await == AgentState::Idle {
                return;
            }
            notified.as_mut().await;
        }
    }

    /// Reset the agent: abort any in-flight run, then clear messages and queues.
    pub async fn reset(&self) {
        self.cancel_all_chat_turns().await;
        self.abort().await;
        self.wait_for_current_run_idle().await;
        self.wait_for_queued_turns_to_drain().await;

        #[cfg(feature = "agent")]
        self.human_interaction_coordinator.cancel_all().await;

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

    async fn cancel_all_chat_turns(&self) {
        let turn_ids = self
            .chat_projector
            .lock()
            .ok()
            .map(|projector| {
                projector
                    .read_snapshot()
                    .threads
                    .into_iter()
                    .flat_map(|thread| thread.turns)
                    .filter(|turn| matches!(turn.status, TurnStatus::Queued | TurnStatus::Running))
                    .map(|turn| turn.turn_id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for turn_id in turn_ids {
            let _ = self.cancel_turn(turn_id).await;
        }
    }

    /// Wait until the agent is idle.
    ///
    /// Returns immediately if already idle; otherwise blocks until the
    /// current run completes, fails, or is aborted.
    pub async fn wait_for_idle(&self) {
        loop {
            let idle_notified = self.idle_notify.notified();
            let queued_turn_notified = self.queued_turn_notify.notified();
            tokio::pin!(idle_notified);
            tokio::pin!(queued_turn_notified);
            let _ = idle_notified.as_mut().enable();
            let _ = queued_turn_notified.as_mut().enable();

            if *self.state.lock().await == AgentState::Idle
                && self.queued_turn_count_for_wait() == 0
            {
                return;
            }
            tokio::select! {
                () = idle_notified.as_mut() => {}
                () = queued_turn_notified.as_mut() => {}
            }
        }
    }
}
