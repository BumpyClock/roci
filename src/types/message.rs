//! Message types for model communication.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelMessage {
    pub role: Role,
    pub content: Vec<ContentPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
}

impl ModelMessage {
    /// Create a system message.
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![ContentPart::Text { text: text.into() }],
            name: None,
            timestamp: Some(Utc::now()),
        }
    }

    /// Create a user message.
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentPart::Text { text: text.into() }],
            name: None,
            timestamp: Some(Utc::now()),
        }
    }

    /// Create an assistant message.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentPart::Text { text: text.into() }],
            name: None,
            timestamp: Some(Utc::now()),
        }
    }

    /// Create a tool result message.
    pub fn tool_result(tool_call_id: impl Into<String>, result: serde_json::Value, is_error: bool) -> Self {
        Self {
            role: Role::Tool,
            content: vec![ContentPart::ToolResult(AgentToolResult {
                tool_call_id: tool_call_id.into(),
                result,
                is_error,
            })],
            name: None,
            timestamp: Some(Utc::now()),
        }
    }

    /// Create a user message with image content.
    pub fn user_with_image(text: impl Into<String>, image_data: String, mime_type: String) -> Self {
        Self {
            role: Role::User,
            content: vec![
                ContentPart::Text { text: text.into() },
                ContentPart::Image(ImageContent { data: image_data, mime_type }),
            ],
            name: None,
            timestamp: Some(Utc::now()),
        }
    }

    /// Extract the text content, concatenating all text parts.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// Extract tool calls from this message.
    pub fn tool_calls(&self) -> Vec<&AgentToolCall> {
        self.content
            .iter()
            .filter_map(|part| match part {
                ContentPart::ToolCall(tc) => Some(tc),
                _ => None,
            })
            .collect()
    }
}

/// Conversation role.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A single part of message content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    Image(ImageContent),
    ToolCall(AgentToolCall),
    ToolResult(AgentToolResult),
}

/// Image content embedded in a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageContent {
    pub data: String,
    pub mime_type: String,
}

/// A tool call requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// A tool execution result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentToolResult {
    pub tool_call_id: String,
    pub result: serde_json::Value,
    #[serde(default)]
    pub is_error: bool,
}
