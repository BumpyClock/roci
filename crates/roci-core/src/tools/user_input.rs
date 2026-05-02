//! User input types for blocking `ask_user` capability.
//!
//! Provides canonical semantic prompt types for parent-mediated user input in
//! the agent loop. The `ask_user` tool is blocking: execution pauses until the
//! parent submits a response or timeout expires.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a user input request.
pub type UserInputRequestId = Uuid;

/// A semantic user input request carried through the human interaction coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserInputRequest {
    /// Unique identifier for this request.
    pub request_id: UserInputRequestId,
    /// Tool call ID from the model.
    pub tool_call_id: String,
    /// Prompt to present to the user.
    pub prompt: AskUserPrompt,
    /// Optional timeout in milliseconds.
    pub timeout_ms: Option<u64>,
}

/// Semantic prompt variants supported by `ask_user`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskUserPrompt {
    /// Free-form text answer.
    Question {
        /// Stable prompt id.
        id: String,
        /// Question text to display.
        question: String,
        /// Optional placeholder text.
        #[serde(default)]
        placeholder: Option<String>,
        /// Optional default answer.
        #[serde(default)]
        default: Option<String>,
        /// Whether the host should allow multiple lines.
        #[serde(default)]
        multiline: bool,
    },
    /// Boolean confirmation.
    Confirm {
        /// Stable prompt id.
        id: String,
        /// Confirmation question text.
        question: String,
        /// Optional default decision.
        #[serde(default)]
        default: Option<bool>,
    },
    /// Single choice from a list.
    Choice {
        /// Stable prompt id.
        id: String,
        /// Choice question text.
        question: String,
        /// Available choices.
        choices: Vec<AskUserChoice>,
        /// Optional default choice id.
        #[serde(default)]
        default: Option<String>,
    },
    /// Multiple choices from a list.
    MultiChoice {
        /// Stable prompt id.
        id: String,
        /// Multi-choice question text.
        question: String,
        /// Available choices.
        choices: Vec<AskUserChoice>,
        /// Optional default choice ids.
        #[serde(default)]
        default: Vec<String>,
        /// Optional minimum selected count.
        #[serde(default)]
        min_selected: Option<usize>,
        /// Optional maximum selected count.
        #[serde(default)]
        max_selected: Option<usize>,
    },
    /// Structured form with typed fields.
    Form {
        /// Stable prompt id.
        id: String,
        /// Optional form title.
        #[serde(default)]
        title: Option<String>,
        /// Form fields.
        fields: Vec<AskUserFormField>,
    },
}

impl AskUserPrompt {
    /// Return stable prompt id.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Question { id, .. }
            | Self::Confirm { id, .. }
            | Self::Choice { id, .. }
            | Self::MultiChoice { id, .. }
            | Self::Form { id, .. } => id,
        }
    }
}

/// Choice option for semantic prompts and form fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserChoice {
    /// Stable choice id.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Optional help text.
    #[serde(default)]
    pub description: Option<String>,
}

/// A typed form field in an `ask_user` form prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskUserFormField {
    /// Stable field id.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Input control kind for this field.
    pub input_kind: AskUserFormInputKind,
    /// Whether the field must be answered.
    #[serde(default)]
    pub required: bool,
    /// Optional placeholder text.
    #[serde(default)]
    pub placeholder: Option<String>,
    /// Optional default value.
    #[serde(default)]
    pub default: Option<UserInputValue>,
    /// Choices for `choice` and `multi_choice` fields.
    #[serde(default)]
    pub choices: Vec<AskUserChoice>,
}

/// Input kind for a form field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskUserFormInputKind {
    Text,
    Boolean,
    Number,
    Choice,
    MultiChoice,
}

/// Response to a user input request, submitted via `submit_user_input`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserInputResponse {
    /// The request ID this response corresponds to.
    pub request_id: UserInputRequestId,
    /// Typed response payload.
    pub result: UserInputResult,
}

/// Typed `ask_user` response payload returned to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UserInputResult {
    /// Free-form question answer.
    Question { answer: String },
    /// Boolean confirmation answer.
    Confirm { confirmed: bool },
    /// Selected choice id.
    Choice { choice: String },
    /// Selected choice ids.
    MultiChoice { choices: Vec<String> },
    /// Form values keyed by field id.
    Form {
        values: BTreeMap<String, UserInputValue>,
    },
    /// User canceled the prompt.
    Canceled,
}

/// Typed value returned by a form prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum UserInputValue {
    Text(String),
    Boolean(bool),
    Number(f64),
    Choice(String),
    MultiChoice(Vec<String>),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serialization_roundtrip() {
        let request = UserInputRequest {
            request_id: Uuid::nil(),
            tool_call_id: "call_123".to_string(),
            prompt: AskUserPrompt::Choice {
                id: "unit".to_string(),
                question: "C or F?".to_string(),
                choices: vec![
                    AskUserChoice {
                        id: "c".to_string(),
                        label: "Celsius".to_string(),
                        description: None,
                    },
                    AskUserChoice {
                        id: "f".to_string(),
                        label: "Fahrenheit".to_string(),
                        description: Some("Imperial temperature unit".to_string()),
                    },
                ],
                default: Some("c".to_string()),
            },
            timeout_ms: Some(30_000),
        };

        let json = serde_json::to_string(&request).expect("serialize");
        let decoded: UserInputRequest = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(decoded, request);
        assert_eq!(decoded.prompt.id(), "unit");
    }

    #[test]
    fn form_response_serialization_roundtrip() {
        let mut values = BTreeMap::new();
        values.insert(
            "name".to_string(),
            UserInputValue::Text("Alice".to_string()),
        );
        values.insert("enabled".to_string(), UserInputValue::Boolean(true));

        let response = UserInputResponse {
            request_id: Uuid::nil(),
            result: UserInputResult::Form { values },
        };

        let json = serde_json::to_string(&response).expect("serialize");
        let decoded: UserInputResponse = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(decoded, response);
    }

    #[test]
    fn canceled_response_serialization() {
        let response = UserInputResponse {
            request_id: Uuid::nil(),
            result: UserInputResult::Canceled,
        };

        let json = serde_json::to_string(&response).expect("serialize");
        let decoded: UserInputResponse = serde_json::from_str(&json).expect("deserialize");

        assert!(matches!(decoded.result, UserInputResult::Canceled));
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
