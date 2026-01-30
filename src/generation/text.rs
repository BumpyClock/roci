//! Text generation with tool loop.

use tracing::{debug, warn};

use crate::error::RociError;
use crate::provider::{ModelProvider, ProviderRequest, ToolDefinition};
use crate::tools::tool::{Tool, ToolExecutionContext};
use crate::tools::arguments::ToolArguments;
use crate::types::*;

/// Maximum tool loop iterations to prevent infinite loops.
const MAX_TOOL_ITERATIONS: usize = 20;

/// Generate text with an optional tool loop.
///
/// If the model returns tool calls, they are executed and fed back
/// until the model produces a final text response or we hit the iteration limit.
pub async fn generate_text(
    provider: &dyn ModelProvider,
    mut messages: Vec<ModelMessage>,
    settings: GenerationSettings,
    tools: &[Box<dyn Tool>],
) -> Result<GenerateTextResult, RociError> {
    let tool_defs: Option<Vec<ToolDefinition>> = if tools.is_empty() {
        None
    } else {
        Some(
            tools
                .iter()
                .map(|t| ToolDefinition {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters: t.parameters().schema.clone(),
                })
                .collect(),
        )
    };

    let mut steps = Vec::new();
    let mut total_usage = Usage::default();

    for iteration in 0..MAX_TOOL_ITERATIONS {
        let request = ProviderRequest {
            messages: messages.clone(),
            settings: settings.clone(),
            tools: tool_defs.clone(),
            response_format: settings.response_format.clone(),
        };

        debug!(iteration, "generate_text: calling provider");
        let response = provider.generate_text(&request).await?;

        total_usage.merge(&response.usage);

        let has_tool_calls = !response.tool_calls.is_empty();

        let mut step = GenerationStep {
            text: response.text.clone(),
            tool_calls: response.tool_calls.clone(),
            tool_results: Vec::new(),
            usage: response.usage,
            finish_reason: response.finish_reason,
        };

        if has_tool_calls {
            // Add assistant message with tool calls
            let mut assistant_content: Vec<ContentPart> = Vec::new();
            if !response.text.is_empty() {
                assistant_content.push(ContentPart::Text {
                    text: response.text.clone(),
                });
            }
            for tc in &response.tool_calls {
                assistant_content.push(ContentPart::ToolCall(tc.clone()));
            }
            messages.push(ModelMessage {
                role: Role::Assistant,
                content: assistant_content,
                name: None,
                timestamp: Some(chrono::Utc::now()),
            });

            // Execute each tool call
            let ctx = ToolExecutionContext::default();
            for tc in &response.tool_calls {
                let tool = tools.iter().find(|t| t.name() == tc.name);
                let result = match tool {
                    Some(t) => {
                        let args = ToolArguments::new(tc.arguments.clone());
                        match t.execute(&args, &ctx).await {
                            Ok(val) => message::AgentToolResult {
                                tool_call_id: tc.id.clone(),
                                result: val,
                                is_error: false,
                            },
                            Err(e) => {
                                warn!(tool = tc.name, error = %e, "Tool execution failed");
                                message::AgentToolResult {
                                    tool_call_id: tc.id.clone(),
                                    result: serde_json::json!({"error": e.to_string()}),
                                    is_error: true,
                                }
                            }
                        }
                    }
                    None => {
                        warn!(tool = tc.name, "Tool not found");
                        message::AgentToolResult {
                            tool_call_id: tc.id.clone(),
                            result: serde_json::json!({"error": format!("Tool '{}' not found", tc.name)}),
                            is_error: true,
                        }
                    }
                };
                step.tool_results.push(result.clone());
                messages.push(ModelMessage::tool_result(
                    result.tool_call_id.clone(),
                    result.result,
                    result.is_error,
                ));
            }

            steps.push(step);
            continue;
        }

        // No tool calls — final response
        steps.push(step);

        return Ok(GenerateTextResult {
            text: response.text,
            steps,
            messages,
            usage: total_usage,
            finish_reason: response.finish_reason,
        });
    }

    // Hit max iterations
    let last_text = steps.last().map(|s| s.text.clone()).unwrap_or_default();
    Ok(GenerateTextResult {
        text: last_text,
        steps,
        messages,
        usage: total_usage,
        finish_reason: Some(FinishReason::Length),
    })
}
