//! Bridge MCP tools into the Roci tool system.

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::error::RociError;
use crate::tools::arguments::ToolArguments;
use crate::tools::dynamic::{DynamicTool, DynamicToolProvider};
use crate::tools::tool::ToolExecutionContext;
use crate::tools::types::AgentToolParameters;

use super::client::{MCPClient, MCPToolCallResult};
use super::schema::MCPToolSchema;

#[async_trait]
trait MCPClientOps: Send {
    async fn initialize(&mut self) -> Result<(), RociError>;
    async fn list_tools(&mut self) -> Result<Vec<MCPToolSchema>, RociError>;
    async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<MCPToolCallResult, RociError>;
}

#[async_trait]
impl MCPClientOps for MCPClient {
    async fn initialize(&mut self) -> Result<(), RociError> {
        MCPClient::initialize(self).await
    }

    async fn list_tools(&mut self) -> Result<Vec<MCPToolSchema>, RociError> {
        MCPClient::list_tools(self).await
    }

    async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<MCPToolCallResult, RociError> {
        MCPClient::call_tool(self, name, arguments).await
    }
}

/// Adapts an MCP client to the DynamicToolProvider trait.
pub struct MCPToolAdapter {
    client: Mutex<Box<dyn MCPClientOps>>,
}

impl MCPToolAdapter {
    pub fn new(client: MCPClient) -> Self {
        Self {
            client: Mutex::new(Box::new(client)),
        }
    }

    #[cfg(test)]
    fn from_client_ops(client: Box<dyn MCPClientOps>) -> Self {
        Self {
            client: Mutex::new(client),
        }
    }
}

#[async_trait]
impl DynamicToolProvider for MCPToolAdapter {
    async fn list_tools(&self) -> Result<Vec<DynamicTool>, RociError> {
        let mut client = self.client.lock().await;
        client.initialize().await?;
        let tools = client.list_tools().await?;
        Ok(tools.into_iter().map(map_mcp_tool_to_dynamic).collect())
    }

    async fn execute_tool(
        &self,
        name: &str,
        args: &ToolArguments,
        _ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError> {
        let mut client = self.client.lock().await;
        client.initialize().await?;
        let result = client.call_tool(name, args.raw().clone()).await?;
        Ok(result.into_value_or_text())
    }
}

fn map_mcp_tool_to_dynamic(tool: MCPToolSchema) -> DynamicTool {
    DynamicTool {
        name: tool.name,
        description: tool.description.unwrap_or_default(),
        parameters: AgentToolParameters::from_schema(tool.input_schema),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::VecDeque;

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

    struct MockClientOps {
        initialize_error: Option<String>,
        list_tools_result: Result<Vec<MCPToolSchema>, String>,
        call_tool_results: VecDeque<Result<MCPToolCallResult, RociError>>,
    }

    #[async_trait]
    impl MCPClientOps for MockClientOps {
        async fn initialize(&mut self) -> Result<(), RociError> {
            match &self.initialize_error {
                Some(message) => Err(RociError::UnsupportedOperation(message.clone())),
                None => Ok(()),
            }
        }

        async fn list_tools(&mut self) -> Result<Vec<MCPToolSchema>, RociError> {
            match &self.list_tools_result {
                Ok(tools) => Ok(tools.clone()),
                Err(message) => Err(RociError::Provider {
                    provider: "mcp".into(),
                    message: message.clone(),
                }),
            }
        }

        async fn call_tool(
            &mut self,
            _name: &str,
            _arguments: serde_json::Value,
        ) -> Result<MCPToolCallResult, RociError> {
            self.call_tool_results
                .pop_front()
                .unwrap_or_else(|| Err(RociError::Stream("missing mock call_tool result".into())))
        }
    }

    #[test]
    fn map_mcp_tool_to_dynamic_preserves_schema() {
        let dynamic = map_mcp_tool_to_dynamic(MCPToolSchema {
            name: "search".into(),
            description: Some("query index".into()),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "q": { "type": "string" }
                }
            }),
        });

        assert_eq!(dynamic.name, "search");
        assert_eq!(dynamic.description, "query index");
        assert_eq!(dynamic.parameters.schema["type"], "object");
    }

    #[tokio::test]
    async fn execute_tool_without_session_errors() {
        let adapter = MCPToolAdapter::new(MCPClient::new(Box::new(NoopTransport)));
        let err = adapter
            .execute_tool(
                "search",
                &ToolArguments::new(json!({"q":"rust"})),
                &ToolExecutionContext::default(),
            )
            .await
            .expect_err("adapter should fail without rmcp running session");

        assert!(matches!(err, RociError::UnsupportedOperation(_)));
    }

    #[tokio::test]
    async fn execute_tool_propagates_tool_error_without_panic() {
        let adapter = MCPToolAdapter::from_client_ops(Box::new(MockClientOps {
            initialize_error: None,
            list_tools_result: Ok(Vec::new()),
            call_tool_results: VecDeque::from([Err(RociError::ToolExecution {
                tool_name: "search".into(),
                message: "downstream tool failure".into(),
            })]),
        }));

        let err = adapter
            .execute_tool(
                "search",
                &ToolArguments::new(json!({"q":"rust"})),
                &ToolExecutionContext::default(),
            )
            .await
            .expect_err("tool errors should be propagated");

        assert!(matches!(
            err,
            RociError::ToolExecution { tool_name, message }
            if tool_name == "search" && message.contains("downstream tool failure")
        ));
    }
}
