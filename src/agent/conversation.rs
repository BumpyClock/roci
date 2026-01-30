//! Conversation message history management.

use crate::types::ModelMessage;

/// Manages a conversation's message history.
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    messages: Vec<ModelMessage>,
}

impl Conversation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a user message.
    pub fn add_user_message(&mut self, text: impl Into<String>) {
        self.messages.push(ModelMessage::user(text));
    }

    /// Add an assistant message.
    pub fn add_assistant_message(&mut self, text: impl Into<String>) {
        self.messages.push(ModelMessage::assistant(text));
    }

    /// Add a raw message.
    pub fn add_message(&mut self, message: ModelMessage) {
        self.messages.push(message);
    }

    /// Get all messages.
    pub fn messages(&self) -> &[ModelMessage] {
        &self.messages
    }

    /// Get the last N messages.
    pub fn last_n(&self, n: usize) -> &[ModelMessage] {
        let start = self.messages.len().saturating_sub(n);
        &self.messages[start..]
    }

    /// Clear all messages.
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Number of messages.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}
