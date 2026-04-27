use std::sync::Arc;

use super::AgentRuntime;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::tools::dynamic::DynamicToolProvider;
use crate::tools::tool::Tool;
use crate::types::ModelMessage;

impl AgentRuntime {
    /// Replace the configured system prompt.
    ///
    /// Runtime mutators are allowed only when idle. This method fails fast if a
    /// run is active or the runtime state lock is contended.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn set_system_prompt(&self, prompt: impl Into<String>) -> Result<(), RociError> {
        let _state_guard = self.lock_state_for_idle_mutation()?;
        let mut system_prompt = self.system_prompt.try_lock().map_err(|_| {
            RociError::InvalidState("Agent is busy (system prompt lock contended)".into())
        })?;
        *system_prompt = Some(prompt.into());
        Ok(())
    }

    /// Replace the configured model used for subsequent runs.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn set_model(&self, model: LanguageModel) -> Result<(), RociError> {
        let _state_guard = self.lock_state_for_idle_mutation()?;
        let mut runtime_model = self
            .model
            .try_lock()
            .map_err(|_| RociError::InvalidState("Agent is busy (model lock contended)".into()))?;
        *runtime_model = model;
        Ok(())
    }

    /// Clear the configured system prompt.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn clear_system_prompt(&self) -> Result<(), RociError> {
        let _state_guard = self.lock_state_for_idle_mutation()?;
        let mut system_prompt = self.system_prompt.try_lock().map_err(|_| {
            RociError::InvalidState("Agent is busy (system prompt lock contended)".into())
        })?;
        *system_prompt = None;
        Ok(())
    }

    /// Replace the full conversation message history.
    ///
    /// This is an atomic replace operation and does not enqueue a run.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn replace_messages(&self, messages: Vec<ModelMessage>) -> Result<(), RociError> {
        let state_guard = self.lock_state_for_idle_mutation()?;
        let mut existing_messages = self.messages.try_lock().map_err(|_| {
            RociError::InvalidState("Agent is busy (messages lock contended)".into())
        })?;
        let snapshot = self
            .chat_projector
            .lock()
            .map_err(|_| RociError::InvalidState("chat projector lock poisoned".into()))?
            .bootstrap_thread(messages.clone())
            .map_err(Self::map_chat_projection_error)?;
        self.runtime_event_store
            .invalidate_thread(snapshot.thread_id, snapshot.last_seq)
            .await
            .map_err(Self::map_chat_projection_error)?;
        *existing_messages = messages;
        drop(existing_messages);
        drop(state_guard);
        self.broadcast_snapshot().await;
        Ok(())
    }

    /// Replace the tool registry used for subsequent runs.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn set_tools(&self, tools: Vec<Arc<dyn Tool>>) -> Result<(), RociError> {
        let _state_guard = self.lock_state_for_idle_mutation()?;
        let mut runtime_tools = self
            .tools
            .try_lock()
            .map_err(|_| RociError::InvalidState("Agent is busy (tools lock contended)".into()))?;
        *runtime_tools = tools;
        Ok(())
    }

    /// Replace the dynamic tool providers used for subsequent runs.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn set_dynamic_tool_providers(
        &self,
        providers: Vec<Arc<dyn DynamicToolProvider>>,
    ) -> Result<(), RociError> {
        let _state_guard = self.lock_state_for_idle_mutation()?;
        let mut runtime_providers = self.dynamic_tool_providers.try_lock().map_err(|_| {
            RociError::InvalidState("Agent is busy (dynamic tool lock contended)".into())
        })?;
        *runtime_providers = providers;
        Ok(())
    }

    /// Clear all dynamic tool providers.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn clear_dynamic_tool_providers(&self) -> Result<(), RociError> {
        self.set_dynamic_tool_providers(Vec::new()).await
    }

    /// Clear all queued steering messages.
    pub async fn clear_steering_queue(&self) {
        self.steering_queue.lock().await.clear();
    }

    /// Clear all queued follow-up messages.
    pub async fn clear_follow_up_queue(&self) {
        self.follow_up_queue.lock().await.clear();
    }

    /// Clear both steering and follow-up queues.
    pub async fn clear_all_queues(&self) {
        self.steering_queue.lock().await.clear();
        self.follow_up_queue.lock().await.clear();
    }

    /// Returns true when either steering or follow-up queue has at least one message.
    pub async fn has_queued_messages(&self) -> bool {
        !self.steering_queue.lock().await.is_empty()
            || !self.follow_up_queue.lock().await.is_empty()
    }
}
