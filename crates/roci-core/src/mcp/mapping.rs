//! MCP result, content, and schema mapping functions.

use crate::error::RociError;
use rmcp::model::{CallToolResult, Content, JsonObject, ResourceContents};

use super::client::MCPToolCallResult;
use super::schema::MCPToolSchema;

pub(super) fn map_mcp_tool_schema(tool: rmcp::model::Tool) -> MCPToolSchema {
    MCPToolSchema {
        name: tool.name.to_string(),
        description: tool.description.map(|d| d.to_string()),
        input_schema: serde_json::Value::Object((*tool.input_schema).clone()),
    }
}

pub(super) fn coerce_tool_arguments(
    value: serde_json::Value,
) -> Result<Option<JsonObject>, RociError> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Object(map) => Ok(Some(map)),
        serde_json::Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            let parsed: serde_json::Value = serde_json::from_str(trimmed).map_err(|e| {
                RociError::InvalidArgument(format!("MCP tool arguments must be valid JSON: {e}"))
            })?;
            coerce_tool_arguments(parsed)
        }
        other => Err(RociError::InvalidArgument(format!(
            "MCP tool arguments must be a JSON object; got {other}"
        ))),
    }
}

fn extract_text_content(content: &[Content]) -> Option<String> {
    let mut lines = Vec::new();
    for item in content {
        if let Some(text) = item.as_text() {
            lines.push(text.text.clone());
            continue;
        }
        if let Some(resource) = item.as_resource() {
            if let ResourceContents::TextResourceContents { text, .. } = &resource.resource {
                lines.push(text.clone());
            }
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

pub(super) fn map_call_result(
    name: &str,
    result: CallToolResult,
) -> Result<MCPToolCallResult, RociError> {
    let text_content = extract_text_content(&result.content);
    let content = result
        .content
        .iter()
        .filter_map(|item| serde_json::to_value(item).ok())
        .collect::<Vec<_>>();

    if result.is_error.unwrap_or(false) {
        let message = result
            .structured_content
            .as_ref()
            .map(|v| v.to_string())
            .or_else(|| text_content.clone())
            .unwrap_or_else(|| "MCP tool returned an error result".into());

        return Err(RociError::ToolExecution {
            tool_name: name.to_string(),
            message,
        });
    }

    Ok(MCPToolCallResult {
        structured_content: result.structured_content,
        text_content,
        content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn coerce_tool_arguments_accepts_object_and_stringified_object() {
        let from_obj = coerce_tool_arguments(json!({"city":"nyc"}))
            .expect("object arguments should parse")
            .expect("object should be present");
        assert_eq!(from_obj.get("city"), Some(&json!("nyc")));

        let from_str = coerce_tool_arguments(json!(r#"{"city":"la"}"#))
            .expect("stringified object should parse")
            .expect("object should be present");
        assert_eq!(from_str.get("city"), Some(&json!("la")));
    }

    #[test]
    fn coerce_tool_arguments_rejects_non_object() {
        let err =
            coerce_tool_arguments(json!(["bad"])).expect_err("array arguments should be rejected");
        assert!(matches!(err, RociError::InvalidArgument(_)));
    }

    #[test]
    fn coerce_tool_arguments_rejects_malformed_json_string() {
        let err = coerce_tool_arguments(json!(r#"{"city":"nyc""#))
            .expect_err("malformed JSON string should be rejected");
        assert!(
            matches!(err, RociError::InvalidArgument(message) if message.contains("valid JSON"))
        );
    }

    #[test]
    fn map_mcp_tool_schema_copies_fields() {
        let mut schema = serde_json::Map::new();
        schema.insert("type".into(), json!("object"));
        let tool = rmcp::model::Tool::new("weather", "lookup weather", schema);

        let mapped = map_mcp_tool_schema(tool);
        assert_eq!(mapped.name, "weather");
        assert_eq!(mapped.description.as_deref(), Some("lookup weather"));
        assert_eq!(mapped.input_schema["type"], "object");
    }

    #[test]
    fn map_call_result_returns_tool_execution_error_for_error_payload() {
        let result: rmcp::model::CallToolResult = serde_json::from_value(json!({
            "content": [
                { "type": "text", "text": "tool failed at runtime" }
            ],
            "structuredContent": {
                "code": "TOOL_FAILURE"
            },
            "isError": true
        }))
        .expect("fixture call result should deserialize");

        let err = map_call_result("search_docs", result)
            .expect_err("error result should map to tool execution error");
        assert!(matches!(
            err,
            RociError::ToolExecution { tool_name, message }
            if tool_name == "search_docs" && message.contains("TOOL_FAILURE")
        ));
    }
}
