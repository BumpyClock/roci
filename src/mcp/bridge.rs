//! Bridge MCP tools into the Roci tool system.

use async_trait::async_trait;

use crate::error::RociError;
use crate::tools::arguments::ToolArguments;
use crate::tools::dynamic::{DynamicTool, DynamicToolProvider};
use crate::tools::tool::ToolExecutionContext;

use super::client::MCPClient;

/// Adapts an MCP client to the DynamicToolProvider trait.
pub struct MCPToolAdapter {
    _client: MCPClient,
}

impl MCPToolAdapter {
    pub fn new(client: MCPClient) -> Self {
        Self { _client: client }
    }
}

#[async_trait]
impl DynamicToolProvider for MCPToolAdapter {
    async fn list_tools(&self) -> Result<Vec<DynamicTool>, RociError> {
        Err(RociError::UnsupportedOperation(
            "MCP tool adapter not yet implemented".into(),
        ))
    }

    async fn execute_tool(
        &self,
        _name: &str,
        _args: &ToolArguments,
        _ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError> {
        Err(RociError::UnsupportedOperation(
            "MCP tool execution not yet implemented".into(),
        ))
    }
}
