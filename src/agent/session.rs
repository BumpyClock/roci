//! Agent session management.

use std::collections::HashMap;

use super::conversation::Conversation;

/// Manages multiple named agent sessions.
#[derive(Debug, Default)]
pub struct AgentSessionManager {
    sessions: HashMap<String, Conversation>,
}

impl AgentSessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get or create a session by ID.
    pub fn get_or_create(&mut self, session_id: &str) -> &mut Conversation {
        self.sessions.entry(session_id.to_string()).or_default()
    }

    /// Get an existing session.
    pub fn get(&self, session_id: &str) -> Option<&Conversation> {
        self.sessions.get(session_id)
    }

    /// Remove a session.
    pub fn remove(&mut self, session_id: &str) -> Option<Conversation> {
        self.sessions.remove(session_id)
    }

    /// List session IDs.
    pub fn session_ids(&self) -> Vec<&str> {
        self.sessions.keys().map(|k| k.as_str()).collect()
    }
}
