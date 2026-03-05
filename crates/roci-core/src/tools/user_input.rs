//! User input types for blocking `ask_user` capability.
//!
//! Provides canonical types for parent-mediated user input in the agent loop.
//! The `ask_user` tool is blocking: execution pauses until the parent submits
//! a response or timeout expires.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a user input request.
pub type UserInputRequestId = Uuid;

/// A request for user input, emitted via `AgentEvent::UserInputRequested`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputRequest {
    /// Unique identifier for this request.
    pub request_id: UserInputRequestId,
    /// Tool call ID from the model.
    pub tool_call_id: String,
    /// Questions to ask the user.
    pub questions: Vec<Question>,
    /// Optional timeout in milliseconds.
    pub timeout_ms: Option<u64>,
}

/// A single question in a user input request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    /// Unique identifier for this question within the request.
    pub id: String,
    /// The question text to display to the user.
    pub text: String,
    /// Optional predefined options for the user to choose from.
    #[serde(default)]
    pub options: Option<Vec<QuestionOption>>,
}

/// A predefined option for a question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    /// Unique identifier for this option.
    pub id: String,
    /// Display label for this option.
    pub label: String,
}

/// Response to a user input request, submitted via `submit_user_input`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputResponse {
    /// The request ID this response corresponds to.
    pub request_id: UserInputRequestId,
    /// Answers to the questions.
    #[serde(default)]
    pub answers: Vec<Answer>,
    /// Whether the user canceled the request.
    #[serde(default)]
    pub canceled: bool,
}

/// An answer to a single question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    /// The question ID this answer corresponds to.
    pub question_id: String,
    /// The answer content (free text or selected option ID).
    pub content: String,
}

/// Error returned when submitting a response for an unknown request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnknownUserInputRequest(pub UserInputRequestId);

impl std::fmt::Display for UnknownUserInputRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown user input request: {}", self.0)
    }
}

impl std::error::Error for UnknownUserInputRequest {}

/// Errors that can occur during user input operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserInputError {
    /// The request ID is unknown (already completed, canceled, or never existed).
    UnknownRequest { request_id: UserInputRequestId },
    /// The request timed out before a response was received.
    Timeout { request_id: UserInputRequestId },
    /// The request was canceled.
    Canceled { request_id: UserInputRequestId },
    /// The host cannot present an interactive prompt for this request.
    InteractivePromptUnavailable {
        request_id: UserInputRequestId,
        reason: String,
    },
    /// No callback was configured to handle user input requests.
    NoCallback,
}

impl std::fmt::Display for UserInputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownRequest { request_id } => {
                write!(f, "unknown user input request: {}", request_id)
            }
            Self::Timeout { request_id } => {
                write!(f, "user input request timed out: {}", request_id)
            }
            Self::Canceled { request_id } => {
                write!(f, "user input request canceled: {}", request_id)
            }
            Self::InteractivePromptUnavailable { request_id, reason } => {
                write!(
                    f,
                    "interactive prompt unavailable for user input request {}: {}",
                    request_id, reason
                )
            }
            Self::NoCallback => write!(f, "no user input callback configured"),
        }
    }
}

impl std::error::Error for UserInputError {}

impl From<UnknownUserInputRequest> for UserInputError {
    fn from(value: UnknownUserInputRequest) -> Self {
        Self::UnknownRequest {
            request_id: value.0,
        }
    }
}

/// Callback type for requesting user input from within a tool.
///
/// This is an async function that takes a request and returns a response.
/// The implementation is provided by the runtime (e.g., CLI, TUI, or IDE).
pub type RequestUserInputFn = Arc<
    dyn Fn(
            UserInputRequest,
        )
            -> Pin<Box<dyn Future<Output = Result<UserInputResponse, UserInputError>> + Send>>
        + Send
        + Sync,
>;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serialization_roundtrip() {
        let request = UserInputRequest {
            request_id: Uuid::nil(),
            tool_call_id: "call_123".to_string(),
            questions: vec![
                Question {
                    id: "q1".to_string(),
                    text: "What is your name?".to_string(),
                    options: None,
                },
                Question {
                    id: "q2".to_string(),
                    text: "Choose an option".to_string(),
                    options: Some(vec![
                        QuestionOption {
                            id: "opt_a".to_string(),
                            label: "Option A".to_string(),
                        },
                        QuestionOption {
                            id: "opt_b".to_string(),
                            label: "Option B".to_string(),
                        },
                    ]),
                },
            ],
            timeout_ms: Some(30_000),
        };

        let json = serde_json::to_string(&request).expect("serialize");
        let decoded: UserInputRequest = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(decoded.request_id, request.request_id);
        assert_eq!(decoded.tool_call_id, request.tool_call_id);
        assert_eq!(decoded.questions.len(), 2);
        assert_eq!(decoded.timeout_ms, Some(30_000));
    }

    #[test]
    fn response_serialization_roundtrip() {
        let response = UserInputResponse {
            request_id: Uuid::nil(),
            answers: vec![
                Answer {
                    question_id: "q1".to_string(),
                    content: "Alice".to_string(),
                },
                Answer {
                    question_id: "q2".to_string(),
                    content: "opt_a".to_string(),
                },
            ],
            canceled: false,
        };

        let json = serde_json::to_string(&response).expect("serialize");
        let decoded: UserInputResponse = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(decoded.request_id, response.request_id);
        assert_eq!(decoded.answers.len(), 2);
        assert!(!decoded.canceled);
    }

    #[test]
    fn canceled_response_serialization() {
        let response = UserInputResponse {
            request_id: Uuid::nil(),
            answers: vec![],
            canceled: true,
        };

        let json = serde_json::to_string(&response).expect("serialize");
        let decoded: UserInputResponse = serde_json::from_str(&json).expect("deserialize");

        assert!(decoded.canceled);
        assert!(decoded.answers.is_empty());
    }

    #[test]
    fn error_display() {
        let request_id = Uuid::nil();

        let err = UserInputError::UnknownRequest { request_id };
        assert!(err.to_string().contains("unknown"));

        let err = UserInputError::Timeout { request_id };
        assert!(err.to_string().contains("timed out"));

        let err = UserInputError::Canceled { request_id };
        assert!(err.to_string().contains("canceled"));

        let err = UserInputError::NoCallback;
        assert!(err.to_string().contains("callback"));
    }

    #[test]
    fn unknown_request_error_conversion() {
        let unknown = UnknownUserInputRequest(Uuid::nil());
        let err: UserInputError = unknown.into();
        assert!(matches!(err, UserInputError::UnknownRequest { .. }));
    }
}
