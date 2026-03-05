//! Ask user tool for blocking user input.

use std::sync::Arc;

use roci::error::RociError;
use roci::tools::arguments::ToolArguments;
use roci::tools::tool::{AgentTool, Tool, ToolExecutionContext};
use roci::tools::types::AgentToolParameters;
use roci::tools::{Question, QuestionOption, UserInputRequest};
use uuid::Uuid;

/// Create the `ask_user` tool — requests user input and blocks until response.
///
/// The tool emits a `UserInputRequested` event and waits for the parent
/// to submit a response via `submit_user_input`.
pub fn ask_user_tool() -> Arc<dyn Tool> {
    Arc::new(AgentTool::new(
        "ask_user",
        "Ask the user questions and wait for their response. Use this when you need clarification or input from the user to proceed.",
        AgentToolParameters::object()
            .array("questions", "List of questions to ask the user", true)
            .number("timeout_ms", "Optional timeout in milliseconds (default: use config)", false)
            .build(),
        |args, ctx: ToolExecutionContext| async move {
            execute_ask_user(args, ctx).await
        },
    ))
}

async fn execute_ask_user(
    args: ToolArguments,
    ctx: ToolExecutionContext,
) -> Result<serde_json::Value, RociError> {
    // Parse questions from arguments
    let questions_arr = args.get_array("questions")?;

    let mut questions = Vec::with_capacity(questions_arr.len());
    for (i, q) in questions_arr.iter().enumerate() {
        let obj = q.as_object().ok_or_else(|| {
            RociError::InvalidArgument(format!("questions[{}] must be an object", i))
        })?;

        let id = obj
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("q{}", i));

        let text = obj
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                RociError::InvalidArgument(format!("questions[{}].text is required", i))
            })?;

        let options = obj.get("options").and_then(|v| v.as_array()).map(|arr| {
            arr.iter()
                .enumerate()
                .filter_map(|(j, opt)| {
                    let opt_obj = opt.as_object()?;
                    let id = opt_obj
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("opt{}", j));
                    let label = opt_obj
                        .get("label")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| id.clone());
                    Some(QuestionOption { id, label })
                })
                .collect()
        });

        questions.push(Question { id, text, options });
    }

    // Parse optional timeout
    let timeout_ms = args.raw().get("timeout_ms").and_then(|v| v.as_u64());

    // Build the request
    let request = UserInputRequest {
        request_id: Uuid::new_v4(),
        tool_call_id: ctx.tool_call_id.clone().unwrap_or_default(),
        questions,
        timeout_ms,
    };

    // Check if callback is available - this requires roci-core compiled with agent feature
    // and the callback being wired in the context
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

        // Call the callback and wait for response
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

        // Return the response as JSON
        Ok(serde_json::to_value(&response).unwrap_or_else(|_| {
            serde_json::json!({
                "request_id": response.request_id.to_string(),
                "answers": response.answers,
                "canceled": response.canceled,
            })
        }))
    }

    #[cfg(not(feature = "agent"))]
    {
        // Without agent feature, ask_user always returns an error
        // The callback mechanism requires the agent feature
        let _ = request; // silence unused warning
        Err(RociError::ToolExecution {
            tool_name: "ask_user".into(),
            message: "ask_user requires agent feature to be enabled".into(),
        })
    }
}
