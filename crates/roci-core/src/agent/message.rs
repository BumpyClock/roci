//! AgentMessage trait and types for extensible message handling.
//!
//! The agent loop works with `AgentMessage` values. Standard LLM messages
//! (`ModelMessage`) implement `AgentMessageExt` automatically. Users can also
//! create custom message types for UI-only or metadata messages that are
//! filtered out before sending to the LLM.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::message::{ContentPart, ModelMessage};

pub const COMPACTION_SUMMARY_PREFIX: &str = "<compaction_summary>";
pub const COMPACTION_SUMMARY_SUFFIX: &str = "</compaction_summary>";
pub const BRANCH_SUMMARY_PREFIX: &str = "<branch_summary>";
pub const BRANCH_SUMMARY_SUFFIX: &str = "</branch_summary>";

fn wrap_summary(prefix: &str, summary: &str, suffix: &str) -> String {
    format!("{prefix}\n{summary}\n{suffix}")
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Trait for messages that can participate in the agent loop.
///
/// Standard LLM messages (`ModelMessage`) implement this trait, returning
/// themselves from [`to_llm_message`]. Custom message types (UI artifacts,
/// metadata, notifications) should return `None`.
pub trait AgentMessageExt: Send + Sync + std::fmt::Debug {
    /// Convert this message into an LLM-facing message, if applicable
    fn to_llm_message(&self) -> Option<ModelMessage>;

    /// Timestamp of the message, if available
    fn timestamp(&self) -> Option<DateTime<Utc>>;

    /// Kind identifier for routing and serialization (e.g. `"llm"`, `"artifact"`)
    fn kind(&self) -> &str;
}

impl AgentMessageExt for ModelMessage {
    fn to_llm_message(&self) -> Option<ModelMessage> {
        Some(self.clone())
    }

    fn timestamp(&self) -> Option<DateTime<Utc>> {
        self.timestamp
    }

    fn kind(&self) -> &str {
        "llm"
    }
}

// ---------------------------------------------------------------------------
// Concrete enum (ergonomic default)
// ---------------------------------------------------------------------------

/// Default message type for the agent loop.
///
/// Wraps standard `ModelMessage` values and supports custom message variants
/// via dedicated summary variants and the `Custom` arm.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessage {
    /// Standard LLM message (user, assistant, system, tool)
    Llm(ModelMessage),
    /// Compaction summary message converted to a provider-facing user message
    CompactionSummary {
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<DateTime<Utc>>,
    },
    /// Branch summary message converted to a provider-facing user message
    BranchSummary {
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<DateTime<Utc>>,
    },
    /// Custom message that will not be sent to the LLM
    Custom {
        kind: String,
        data: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<DateTime<Utc>>,
    },
}

impl AgentMessageExt for AgentMessage {
    fn to_llm_message(&self) -> Option<ModelMessage> {
        match self {
            AgentMessage::Llm(msg) => Some(msg.clone()),
            AgentMessage::CompactionSummary { summary, .. } => {
                Some(ModelMessage::user(wrap_summary(
                    COMPACTION_SUMMARY_PREFIX,
                    summary,
                    COMPACTION_SUMMARY_SUFFIX,
                )))
            }
            AgentMessage::BranchSummary { summary, .. } => Some(ModelMessage::user(wrap_summary(
                BRANCH_SUMMARY_PREFIX,
                summary,
                BRANCH_SUMMARY_SUFFIX,
            ))),
            AgentMessage::Custom { .. } => None,
        }
    }

    fn timestamp(&self) -> Option<DateTime<Utc>> {
        match self {
            AgentMessage::Llm(msg) => msg.timestamp,
            AgentMessage::CompactionSummary { timestamp, .. } => *timestamp,
            AgentMessage::BranchSummary { timestamp, .. } => *timestamp,
            AgentMessage::Custom { timestamp, .. } => *timestamp,
        }
    }

    fn kind(&self) -> &str {
        match self {
            AgentMessage::Llm(_) => "llm",
            AgentMessage::CompactionSummary { .. } => "compaction_summary",
            AgentMessage::BranchSummary { .. } => "branch_summary",
            AgentMessage::Custom { kind, .. } => kind,
        }
    }
}

impl AgentMessage {
    /// Create from a `ModelMessage`
    pub fn from_model(msg: ModelMessage) -> Self {
        AgentMessage::Llm(msg)
    }

    /// Create a compaction summary message
    pub fn compaction_summary(summary: impl Into<String>) -> Self {
        AgentMessage::CompactionSummary {
            summary: summary.into(),
            timestamp: Some(Utc::now()),
        }
    }

    /// Create a branch summary message
    pub fn branch_summary(summary: impl Into<String>) -> Self {
        AgentMessage::BranchSummary {
            summary: summary.into(),
            timestamp: Some(Utc::now()),
        }
    }

    /// Create a custom message
    pub fn custom(kind: impl Into<String>, data: serde_json::Value) -> Self {
        AgentMessage::Custom {
            kind: kind.into(),
            data,
            timestamp: Some(Utc::now()),
        }
    }

    /// Shorthand: create a user text message
    pub fn user(text: impl Into<String>) -> Self {
        AgentMessage::Llm(ModelMessage::user(text))
    }

    /// Shorthand: create an assistant text message
    pub fn assistant(text: impl Into<String>) -> Self {
        AgentMessage::Llm(ModelMessage::assistant(text))
    }

    /// Extract text content if this message converts to an LLM message
    pub fn text(&self) -> Option<String> {
        self.to_llm_message().map(|m| m.text())
    }

    /// Extract content parts if this message converts to an LLM message
    pub fn content(&self) -> Option<Vec<ContentPart>> {
        self.to_llm_message().map(|m| m.content)
    }
}

impl From<ModelMessage> for AgentMessage {
    fn from(msg: ModelMessage) -> Self {
        AgentMessage::Llm(msg)
    }
}

// ---------------------------------------------------------------------------
// Conversion helper
// ---------------------------------------------------------------------------

/// Filter a slice of agent messages down to only LLM-compatible messages
///
/// This is called before each LLM request to strip out custom messages
/// (artifacts, notifications, etc.) that the model shouldn't see
pub fn convert_to_llm<M: AgentMessageExt>(messages: &[M]) -> Vec<ModelMessage> {
    messages.iter().filter_map(|m| m.to_llm_message()).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::message::Role;

    #[test]
    fn model_message_implements_trait() {
        let msg = ModelMessage::user("hello");
        assert!(msg.to_llm_message().is_some());
        assert_eq!(msg.kind(), "llm");
    }

    #[test]
    fn agent_message_llm_variant_converts() {
        let msg = AgentMessage::user("hello");
        assert!(msg.to_llm_message().is_some());
        assert_eq!(msg.kind(), "llm");
        assert_eq!(msg.text().unwrap(), "hello");
    }

    #[test]
    fn compaction_summary_variant_converts_to_user_with_wrapper() {
        let msg = AgentMessage::compaction_summary("compact this");

        let llm = msg.to_llm_message().expect("summary should convert");

        assert_eq!(llm.role, Role::User);
        let text = llm.text();
        assert!(text.contains(COMPACTION_SUMMARY_PREFIX));
        assert!(text.contains("compact this"));
        assert!(text.contains(COMPACTION_SUMMARY_SUFFIX));
        assert_eq!(msg.kind(), "compaction_summary");
    }

    #[test]
    fn branch_summary_variant_converts_to_user_with_wrapper() {
        let msg = AgentMessage::branch_summary("branch history");

        let llm = msg.to_llm_message().expect("summary should convert");

        assert_eq!(llm.role, Role::User);
        let text = llm.text();
        assert!(text.contains(BRANCH_SUMMARY_PREFIX));
        assert!(text.contains("branch history"));
        assert!(text.contains(BRANCH_SUMMARY_SUFFIX));
        assert_eq!(msg.kind(), "branch_summary");
    }

    #[test]
    fn agent_message_custom_variant_filters() {
        let msg = AgentMessage::custom("artifact", serde_json::json!({"html": "<h1>Hi</h1>"}));
        assert!(msg.to_llm_message().is_none());
        assert_eq!(msg.kind(), "artifact");
    }

    #[test]
    fn convert_to_llm_filters_custom() {
        let messages = vec![
            AgentMessage::user("hello"),
            AgentMessage::custom("notification", serde_json::json!({})),
            AgentMessage::assistant("world"),
        ];

        let llm = convert_to_llm(&messages);
        assert_eq!(llm.len(), 2);
        assert_eq!(llm[0].role, Role::User);
        assert_eq!(llm[1].role, Role::Assistant);
    }

    #[test]
    fn from_model_message() {
        let model_msg = ModelMessage::user("test");
        let agent_msg: AgentMessage = model_msg.into();
        assert!(agent_msg.to_llm_message().is_some());
    }
}
