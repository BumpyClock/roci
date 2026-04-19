//! Response parsing and API types for the OpenAI Responses API.

use serde::Deserialize;

use roci_core::error::RociError;
use roci_core::types::*;

use roci_core::provider::ProviderResponse;

use super::OpenAiResponsesProvider;

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

impl OpenAiResponsesProvider {
    /// Convert a Responses API payload into a provider response.
    pub(crate) fn parse_response(
        data: ResponsesApiResponse,
    ) -> Result<ProviderResponse, RociError> {
        if let Some(outputs) = data.output {
            let mut text = String::new();
            let mut tool_calls = Vec::new();

            for output in outputs {
                match output.r#type.as_str() {
                    "message" => {
                        if let Some(content) = output.content {
                            for chunk in content {
                                match chunk.r#type.as_str() {
                                    "output_text" => {
                                        if let Some(segment) = chunk.text {
                                            text.push_str(&segment);
                                        }
                                    }
                                    "tool_call" => {
                                        if let Some(tool_call) = chunk.tool_call {
                                            tool_calls.push(Self::convert_tool_call(tool_call));
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    "function_call" => {
                        if let (Some(id), Some(name), Some(args)) =
                            (output.call_id, output.name, output.arguments)
                        {
                            tool_calls.push(Self::convert_flat_tool_call(&id, &name, &args));
                        }
                    }
                    "tool_call" => {
                        if let Some(tool_call) = output.tool_call {
                            tool_calls.push(Self::convert_tool_call(tool_call));
                        }
                    }
                    _ => {}
                }
            }

            let finish_reason = if !tool_calls.is_empty() {
                Some(FinishReason::ToolCalls)
            } else {
                data.status.as_deref().and_then(|s| match s {
                    "completed" => Some(FinishReason::Stop),
                    "incomplete" => Some(FinishReason::Length),
                    _ => None,
                })
            };

            return Ok(ProviderResponse {
                text,
                usage: Self::map_usage(data.usage),
                tool_calls,
                finish_reason,
                thinking: Vec::new(),
            });
        }

        if let Some(choices) = data.choices {
            let choice = choices
                .into_iter()
                .next()
                .ok_or_else(|| RociError::api(200, "No choices in OpenAI response"))?;
            let tool_calls = choice
                .message
                .tool_calls
                .unwrap_or_default()
                .into_iter()
                .map(Self::convert_tool_call)
                .collect::<Vec<_>>();
            let finish_reason = choice
                .finish_reason
                .as_deref()
                .and_then(|reason| match reason {
                    "stop" => Some(FinishReason::Stop),
                    "length" => Some(FinishReason::Length),
                    "tool_calls" => Some(FinishReason::ToolCalls),
                    _ => None,
                });
            let finish_reason = if !tool_calls.is_empty() {
                Some(FinishReason::ToolCalls)
            } else {
                finish_reason
            };

            return Ok(ProviderResponse {
                text: choice.message.content.unwrap_or_default(),
                usage: Self::map_usage(data.usage),
                tool_calls,
                finish_reason,
                thinking: Vec::new(),
            });
        }

        Err(RociError::api(
            200,
            "No output or choices in OpenAI response",
        ))
    }

    pub(crate) fn convert_flat_tool_call(id: &str, name: &str, args: &str) -> AgentToolCall {
        AgentToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: serde_json::from_str(args)
                .unwrap_or(serde_json::Value::String(args.to_string())),
            recipient: None,
        }
    }

    pub(crate) fn convert_tool_call(tool_call: ResponsesToolCall) -> AgentToolCall {
        Self::convert_flat_tool_call(
            &tool_call.id,
            &tool_call.function.name,
            &tool_call.function.arguments,
        )
    }

    pub(crate) fn map_usage(usage: Option<ResponsesUsage>) -> Usage {
        usage
            .map(|u| {
                let input_tokens = u.input_tokens.or(u.prompt_tokens).unwrap_or(0);
                let output_tokens = u.output_tokens.or(u.completion_tokens).unwrap_or(0);
                let total_tokens = u.total_tokens.unwrap_or(input_tokens + output_tokens);
                Usage {
                    input_tokens,
                    output_tokens,
                    total_tokens,
                    ..Default::default()
                }
            })
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// API response serde types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct ResponsesApiResponse {
    pub(crate) output: Option<Vec<ResponsesOutputItem>>,
    pub(crate) choices: Option<Vec<ResponsesChoice>>,
    pub(crate) status: Option<String>,
    pub(crate) usage: Option<ResponsesUsage>,
}

#[derive(Deserialize)]
pub(crate) struct ResponsesOutputItem {
    pub(crate) r#type: String,
    pub(crate) content: Option<Vec<ResponsesOutputContent>>,
    #[serde(default)]
    pub(crate) call_id: Option<String>,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) arguments: Option<String>,
    #[serde(default)]
    pub(crate) tool_call: Option<ResponsesToolCall>,
}

#[derive(Deserialize)]
pub(crate) struct ResponsesOutputContent {
    pub(crate) r#type: String,
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) tool_call: Option<ResponsesToolCall>,
}

#[derive(Deserialize)]
pub(crate) struct ResponsesChoice {
    pub(crate) message: ResponsesChoiceMessage,
    #[serde(default)]
    pub(crate) finish_reason: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ResponsesChoiceMessage {
    #[serde(default)]
    pub(crate) content: Option<String>,
    #[serde(default)]
    pub(crate) tool_calls: Option<Vec<ResponsesToolCall>>,
}

#[derive(Deserialize)]
pub(crate) struct ResponsesToolCall {
    pub(crate) id: String,
    pub(crate) function: ResponsesToolCallFunction,
}

#[derive(Deserialize)]
pub(crate) struct ResponsesToolCallFunction {
    pub(crate) name: String,
    pub(crate) arguments: String,
}

#[derive(Deserialize)]
pub(crate) struct ResponsesUsage {
    #[serde(default)]
    pub(crate) input_tokens: Option<u32>,
    #[serde(default)]
    pub(crate) output_tokens: Option<u32>,
    #[serde(default)]
    pub(crate) prompt_tokens: Option<u32>,
    #[serde(default)]
    pub(crate) completion_tokens: Option<u32>,
    #[serde(default)]
    pub(crate) total_tokens: Option<u32>,
}
