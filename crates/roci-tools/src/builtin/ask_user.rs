//! Ask user tool for blocking semantic user input.

use std::sync::Arc;

use roci::error::RociError;
use roci::tools::arguments::ToolArguments;
use roci::tools::tool::{AgentTool, Tool, ToolApproval, ToolExecutionContext};
use roci::tools::types::AgentToolParameters;
use roci::tools::{
    AskUserChoice, AskUserFormField, AskUserFormInputKind, AskUserPrompt, UserInputRequest,
};
use uuid::Uuid;

/// Create the `ask_user` tool — requests user input and blocks until response.
///
/// The tool emits a human interaction event and waits for the parent
/// to submit a response via `submit_user_input`.
pub fn ask_user_tool() -> Arc<dyn Tool> {
    Arc::new(AgentTool::new(
        "ask_user",
        "Ask the user for semantic input and wait for their response. Use this when you need clarification or input from the user to proceed.",
        ask_user_parameters(),
        |args, ctx: ToolExecutionContext| async move {
            execute_ask_user(args, ctx).await
        },
    )
    .with_approval(ToolApproval::safe_host_input()))
}

fn ask_user_parameters() -> AgentToolParameters {
    AgentToolParameters::from_schema(serde_json::json!({
        "type": "object",
        "properties": {
            "kind": {
                "type": "string",
                "enum": ["question", "confirm", "choice", "multi_choice", "form"],
                "description": "Prompt kind to show the user"
            },
            "id": {
                "type": "string",
                "description": "Stable prompt id, default: input"
            },
            "question": {
                "type": "string",
                "description": "Question text for question, confirm, choice, and multi_choice prompts"
            },
            "placeholder": {
                "type": "string",
                "description": "Optional placeholder for text input"
            },
            "default": {
                "description": "Optional default answer or choice id(s)"
            },
            "multiline": {
                "type": "boolean",
                "description": "Whether a question prompt should accept multiple lines"
            },
            "choices": {
                "type": "array",
                "description": "Choices for choice and multi_choice prompts",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "label": {"type": "string"},
                        "description": {"type": "string"}
                    },
                    "required": ["id", "label"]
                }
            },
            "min_selected": {
                "type": "integer",
                "description": "Optional minimum selected choices for multi_choice"
            },
            "max_selected": {
                "type": "integer",
                "description": "Optional maximum selected choices for multi_choice"
            },
            "title": {
                "type": "string",
                "description": "Optional title for form prompts"
            },
            "fields": {
                "type": "array",
                "description": "Typed fields for form prompts",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "label": {"type": "string"},
                        "input_kind": {
                            "type": "string",
                            "enum": ["text", "boolean", "number", "choice", "multi_choice"]
                        },
                        "required": {"type": "boolean"},
                        "placeholder": {"type": "string"},
                        "default": {},
                        "choices": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": {"type": "string"},
                                    "label": {"type": "string"},
                                    "description": {"type": "string"}
                                },
                                "required": ["id", "label"]
                            }
                        }
                    },
                    "required": ["id", "label", "input_kind"]
                }
            },
            "timeout_ms": {
                "type": "integer",
                "description": "Optional timeout in milliseconds (default: use config)"
            }
        },
        "required": ["kind"]
    }))
}

async fn execute_ask_user(
    args: ToolArguments,
    ctx: ToolExecutionContext,
) -> Result<serde_json::Value, RociError> {
    let prompt = parse_prompt(args.raw())?;
    let timeout_ms = args
        .raw()
        .get("timeout_ms")
        .and_then(serde_json::Value::as_u64);

    let request = UserInputRequest {
        request_id: Uuid::new_v4(),
        tool_call_id: ctx.tool_call_id.clone().unwrap_or_default(),
        prompt,
        timeout_ms,
    };

    #[cfg(feature = "agent")]
    {
        use roci::tools::UserInputError;

        let callback = ctx
            .request_user_input
            .as_ref()
            .ok_or_else(|| RociError::ToolExecution {
                tool_name: "ask_user".into(),
                message: "no user input callback configured".into(),
            })?;

        let response = callback(request).await.map_err(|e| {
            let message = match e {
                UserInputError::UnknownRequest { request_id } => {
                    format!("unknown request: {}", request_id)
                }
                UserInputError::Timeout { request_id } => {
                    format!("request timed out: {}", request_id)
                }
                UserInputError::Canceled { request_id } => {
                    format!("request canceled: {}", request_id)
                }
                UserInputError::InteractivePromptUnavailable { request_id, reason } => {
                    format!(
                        "interactive prompt unavailable for request {}: {}",
                        request_id, reason
                    )
                }
                UserInputError::NoCallback => "no user input callback configured".to_string(),
            };
            RociError::ToolExecution {
                tool_name: "ask_user".into(),
                message,
            }
        })?;

        serde_json::to_value(response).map_err(|error| RociError::ToolExecution {
            tool_name: "ask_user".into(),
            message: format!("failed to serialize ask_user response: {error}"),
        })
    }

    #[cfg(not(feature = "agent"))]
    {
        let _ = request;
        Err(RociError::ToolExecution {
            tool_name: "ask_user".into(),
            message: "ask_user requires agent feature to be enabled".into(),
        })
    }
}

fn parse_prompt(value: &serde_json::Value) -> Result<AskUserPrompt, RociError> {
    let object = value
        .as_object()
        .ok_or_else(|| RociError::InvalidArgument("ask_user arguments must be an object".into()))?;
    let kind = required_str(object, "kind")?;
    let id = optional_str(object, "id").unwrap_or("input").to_string();

    match kind {
        "question" => Ok(AskUserPrompt::Question {
            id,
            question: required_str(object, "question")?.to_string(),
            placeholder: optional_str(object, "placeholder").map(str::to_string),
            default: optional_str(object, "default").map(str::to_string),
            multiline: object
                .get("multiline")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        }),
        "confirm" => Ok(AskUserPrompt::Confirm {
            id,
            question: required_str(object, "question")?.to_string(),
            default: object.get("default").and_then(serde_json::Value::as_bool),
        }),
        "choice" => Ok(AskUserPrompt::Choice {
            id,
            question: required_str(object, "question")?.to_string(),
            choices: parse_required_choices(object, "choices")?,
            default: optional_str(object, "default").map(str::to_string),
        }),
        "multi_choice" => Ok(AskUserPrompt::MultiChoice {
            id,
            question: required_str(object, "question")?.to_string(),
            choices: parse_required_choices(object, "choices")?,
            default: parse_default_choices(object)?,
            min_selected: optional_usize(object, "min_selected")?,
            max_selected: optional_usize(object, "max_selected")?,
        }),
        "form" => Ok(AskUserPrompt::Form {
            id,
            title: optional_str(object, "title").map(str::to_string),
            fields: parse_required_fields(object)?,
        }),
        other => Err(RociError::InvalidArgument(format!(
            "unsupported ask_user kind: {other}"
        ))),
    }
}

fn required_str<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<&'a str, RociError> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| RociError::InvalidArgument(format!("{field} is required")))
}

fn optional_str<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Option<&'a str> {
    object.get(field).and_then(serde_json::Value::as_str)
}

fn optional_usize(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<Option<usize>, RociError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    let raw = value
        .as_u64()
        .ok_or_else(|| RociError::InvalidArgument(format!("{field} must be a positive integer")))?;
    usize::try_from(raw)
        .map(Some)
        .map_err(|_| RociError::InvalidArgument(format!("{field} is too large")))
}

fn parse_required_choices(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<Vec<AskUserChoice>, RociError> {
    let choices = object
        .get(field)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| RociError::InvalidArgument(format!("{field} is required")))?;
    if choices.is_empty() {
        return Err(RociError::InvalidArgument(format!(
            "{field} must include at least one choice"
        )));
    }
    choices
        .iter()
        .enumerate()
        .map(|(index, choice)| parse_choice(choice, &format!("{field}[{index}]")))
        .collect()
}

fn parse_choice(value: &serde_json::Value, path: &str) -> Result<AskUserChoice, RociError> {
    let object = value
        .as_object()
        .ok_or_else(|| RociError::InvalidArgument(format!("{path} must be an object")))?;
    Ok(AskUserChoice {
        id: required_str(object, "id")?.to_string(),
        label: required_str(object, "label")?.to_string(),
        description: optional_str(object, "description").map(str::to_string),
    })
}

fn parse_default_choices(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<String>, RociError> {
    let Some(default) = object.get("default") else {
        return Ok(Vec::new());
    };
    let values = default
        .as_array()
        .ok_or_else(|| RociError::InvalidArgument("default must be an array".into()))?;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value.as_str().map(str::to_string).ok_or_else(|| {
                RociError::InvalidArgument(format!("default[{index}] must be a string"))
            })
        })
        .collect()
}

fn parse_required_fields(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<AskUserFormField>, RociError> {
    let fields = object
        .get("fields")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| RociError::InvalidArgument("fields is required".into()))?;
    if fields.is_empty() {
        return Err(RociError::InvalidArgument(
            "fields must include at least one field".into(),
        ));
    }
    fields
        .iter()
        .enumerate()
        .map(|(index, field)| parse_field(field, &format!("fields[{index}]")))
        .collect()
}

fn parse_field(value: &serde_json::Value, path: &str) -> Result<AskUserFormField, RociError> {
    let object = value
        .as_object()
        .ok_or_else(|| RociError::InvalidArgument(format!("{path} must be an object")))?;
    let input_kind = parse_input_kind(required_str(object, "input_kind")?)?;
    let choices = match input_kind {
        AskUserFormInputKind::Choice | AskUserFormInputKind::MultiChoice => {
            parse_required_choices(object, "choices")?
        }
        _ => Vec::new(),
    };

    Ok(AskUserFormField {
        id: required_str(object, "id")?.to_string(),
        label: required_str(object, "label")?.to_string(),
        input_kind,
        required: object
            .get("required")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        placeholder: optional_str(object, "placeholder").map(str::to_string),
        default: object
            .get("default")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| {
                RociError::InvalidArgument(format!("{path}.default is invalid: {error}"))
            })?,
        choices,
    })
}

fn parse_input_kind(value: &str) -> Result<AskUserFormInputKind, RociError> {
    match value {
        "text" => Ok(AskUserFormInputKind::Text),
        "boolean" => Ok(AskUserFormInputKind::Boolean),
        "number" => Ok(AskUserFormInputKind::Number),
        "choice" => Ok(AskUserFormInputKind::Choice),
        "multi_choice" => Ok(AskUserFormInputKind::MultiChoice),
        other => Err(RociError::InvalidArgument(format!(
            "unsupported input_kind: {other}"
        ))),
    }
}
