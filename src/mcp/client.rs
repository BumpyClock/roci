//! MCP client for connecting to MCP servers.

use crate::error::RociError;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult, Content, JsonObject, ResourceContents},
    service::{ClientInitializeError, DynService, RoleClient, RunningService, ServiceError},
};

use super::transport::MCPTransport;

type DynClientService = Box<dyn DynService<RoleClient>>;
pub type MCPRunningService = RunningService<RoleClient, DynClientService>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MCPConnectionState {
    Disconnected,
    Connected,
    Initialized,
    Closed,
}

#[derive(Debug, Clone)]
pub struct MCPToolCallResult {
    pub structured_content: Option<serde_json::Value>,
    pub text_content: Option<String>,
    pub content: Vec<serde_json::Value>,
}

impl MCPToolCallResult {
    pub fn into_value_or_text(self) -> serde_json::Value {
        if let Some(structured) = self.structured_content {
            return structured;
        }
        if let Some(text) = self.text_content {
            return serde_json::Value::String(text);
        }
        serde_json::Value::Array(self.content)
    }
}

/// Client for a Model Context Protocol server.
pub struct MCPClient {
    transport: Option<Box<dyn MCPTransport>>,
    session: Option<MCPRunningService>,
    state: MCPConnectionState,
}

impl MCPClient {
    /// Create a new MCP client with the given transport.
    pub fn new(transport: Box<dyn MCPTransport>) -> Self {
        Self {
            transport: Some(transport),
            session: None,
            state: MCPConnectionState::Disconnected,
        }
    }

    /// Create a client from an already-running rmcp service.
    ///
    /// Initialization handshake is already handled by rmcp `serve(...)`.
    pub fn from_running_service(session: MCPRunningService) -> Self {
        Self {
            transport: None,
            session: Some(session),
            state: MCPConnectionState::Connected,
        }
    }

    /// Convert an rmcp initialization result into an MCP client.
    pub fn from_running_service_result(
        result: Result<MCPRunningService, ClientInitializeError>,
    ) -> Result<Self, RociError> {
        result
            .map(Self::from_running_service)
            .map_err(map_client_initialize_error)
    }

    /// Attach/replace the active rmcp session.
    pub fn attach_running_service(&mut self, session: MCPRunningService) {
        self.session = Some(session);
        self.state = MCPConnectionState::Connected;
    }

    pub fn connection_state(&self) -> MCPConnectionState {
        self.state
    }

    pub fn is_initialized(&self) -> bool {
        self.state == MCPConnectionState::Initialized
    }

    /// Initialize the MCP connection.
    pub async fn initialize(&mut self) -> Result<(), RociError> {
        if self.state == MCPConnectionState::Initialized {
            return Ok(());
        }

        let Some(session) = self.session.as_ref() else {
            return Err(match self.transport {
                Some(_) => RociError::UnsupportedOperation(
                    "MCP transport is present but no rmcp RunningService is attached".into(),
                ),
                None => RociError::Configuration("Missing MCP session".into()),
            });
        };

        if session.is_closed() {
            self.state = MCPConnectionState::Closed;
            return Err(RociError::Stream("MCP session is closed".into()));
        }

        self.state = MCPConnectionState::Initialized;
        Ok(())
    }

    /// List available tools from the MCP server.
    pub async fn list_tools(&mut self) -> Result<Vec<super::schema::MCPToolSchema>, RociError> {
        self.ensure_initialized()?;
        let session = self.session_ref()?;

        let tools = match session.list_all_tools().await {
            Ok(tools) => tools,
            Err(ServiceError::UnexpectedResponse) => {
                let page = session
                    .list_tools(None)
                    .await
                    .map_err(|e| map_service_error("list_tools", e))?;
                page.tools
            }
            Err(e) => return Err(map_service_error("list_tools", e)),
        };

        Ok(tools.into_iter().map(map_mcp_tool_schema).collect())
    }

    /// Execute a tool on the MCP server.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<MCPToolCallResult, RociError> {
        self.ensure_initialized()?;
        let session = self.session_ref()?;
        let arguments = coerce_tool_arguments(arguments)?;

        let result = session
            .call_tool(CallToolRequestParams {
                meta: None,
                name: name.to_owned().into(),
                arguments,
                task: None,
            })
            .await
            .map_err(|e| map_service_error("call_tool", e))?;

        map_call_result(name, result)
    }

    fn ensure_initialized(&self) -> Result<(), RociError> {
        match self.state {
            MCPConnectionState::Initialized => Ok(()),
            MCPConnectionState::Closed => Err(RociError::Stream("MCP session is closed".into())),
            _ => Err(RociError::UnsupportedOperation(
                "MCP client must be initialized first".into(),
            )),
        }
    }

    fn session_ref(&mut self) -> Result<&mut MCPRunningService, RociError> {
        self.session
            .as_mut()
            .ok_or_else(|| RociError::Configuration("Missing MCP session".into()))
    }
}

fn map_mcp_tool_schema(tool: rmcp::model::Tool) -> super::schema::MCPToolSchema {
    super::schema::MCPToolSchema {
        name: tool.name.to_string(),
        description: tool.description.map(|d| d.to_string()),
        input_schema: serde_json::Value::Object((*tool.input_schema).clone()),
    }
}

fn coerce_tool_arguments(value: serde_json::Value) -> Result<Option<JsonObject>, RociError> {
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

fn map_call_result(name: &str, result: CallToolResult) -> Result<MCPToolCallResult, RociError> {
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

fn map_client_initialize_error(error: ClientInitializeError) -> RociError {
    match error {
        ClientInitializeError::ConnectionClosed(context) => {
            RociError::Stream(format!("MCP initialize connection closed: {context}"))
        }
        ClientInitializeError::TransportError { error, context } => RociError::Stream(format!(
            "MCP initialize transport error ({context}): {error}"
        )),
        ClientInitializeError::JsonRpcError(error) => RociError::Provider {
            provider: "mcp".into(),
            message: format!(
                "MCP initialize JSON-RPC error {}: {}",
                error.code.0, error.message
            ),
        },
        ClientInitializeError::Cancelled => RociError::Stream("MCP initialize cancelled".into()),
        other => RociError::Provider {
            provider: "mcp".into(),
            message: format!("MCP initialize error: {other}"),
        },
    }
}

fn map_service_error(context: &str, error: ServiceError) -> RociError {
    match error {
        ServiceError::McpError(error) => RociError::Provider {
            provider: "mcp".into(),
            message: format!("{context}: MCP error {}: {}", error.code.0, error.message),
        },
        ServiceError::TransportSend(error) => {
            RociError::Stream(format!("{context}: MCP transport send failed: {error}"))
        }
        ServiceError::TransportClosed => {
            RociError::Stream(format!("{context}: MCP transport closed"))
        }
        ServiceError::UnexpectedResponse => RociError::Provider {
            provider: "mcp".into(),
            message: format!("{context}: unexpected MCP response"),
        },
        ServiceError::Cancelled { reason } => {
            let suffix = reason
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default();
            RociError::Stream(format!("{context}: MCP request cancelled{suffix}"))
        }
        ServiceError::Timeout { timeout } => RociError::Timeout(timeout.as_millis() as u64),
        other => RociError::Provider {
            provider: "mcp".into(),
            message: format!("{context}: MCP service error: {other}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::time::Duration;

    use crate::mcp::transport::MCPTransport;

    struct NoopTransport;

    #[async_trait]
    impl MCPTransport for NoopTransport {
        async fn send(&mut self, _message: serde_json::Value) -> Result<(), RociError> {
            Ok(())
        }

        async fn receive(&mut self) -> Result<serde_json::Value, RociError> {
            Ok(serde_json::Value::Null)
        }

        async fn close(&mut self) -> Result<(), RociError> {
            Ok(())
        }
    }

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

    #[tokio::test]
    async fn list_tools_requires_initialize() {
        let mut client = MCPClient::new(Box::new(NoopTransport));
        let err = client
            .list_tools()
            .await
            .expect_err("listing tools should require initialize");
        assert!(matches!(err, RociError::UnsupportedOperation(_)));
    }

    #[test]
    fn map_service_error_protocol_violation_maps_to_provider_error() {
        let err = map_service_error("list_tools", ServiceError::UnexpectedResponse);
        assert!(matches!(
            err,
            RociError::Provider { provider, message }
            if provider == "mcp" && message.contains("unexpected MCP response")
        ));
    }

    #[test]
    fn map_service_error_timeout_maps_to_timeout_error() {
        let err = map_service_error(
            "call_tool",
            ServiceError::Timeout {
                timeout: Duration::from_millis(2750),
            },
        );
        assert!(matches!(err, RociError::Timeout(2750)));
    }

    #[test]
    fn map_service_error_cancelled_reason_is_preserved() {
        let err = map_service_error(
            "call_tool",
            ServiceError::Cancelled {
                reason: Some("client cancelled".into()),
            },
        );
        assert!(matches!(
            err,
            RociError::Stream(message) if message.contains("client cancelled")
        ));
    }

    #[test]
    fn from_running_service_result_maps_jsonrpc_initialize_error() {
        let init_error = ClientInitializeError::JsonRpcError(
            rmcp::model::ErrorData::invalid_request("bad initialize payload", None),
        );
        let err = match MCPClient::from_running_service_result(Err(init_error)) {
            Ok(_) => panic!("initialize error should be mapped"),
            Err(err) => err,
        };
        assert!(matches!(
            err,
            RociError::Provider { provider, message }
            if provider == "mcp"
                && message.contains("JSON-RPC error")
                && message.contains("bad initialize payload")
        ));
    }

    #[test]
    fn map_call_result_returns_tool_execution_error_for_error_payload() {
        let result: CallToolResult = serde_json::from_value(json!({
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
