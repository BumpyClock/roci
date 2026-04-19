//! Shared MCP client operations trait used by adapters and aggregators.

use async_trait::async_trait;

use crate::error::RociError;

use super::client::{MCPClient, MCPToolCallResult};
use super::schema::MCPToolSchema;

/// Internal operations required by MCP adapters and aggregators.
#[async_trait]
pub(super) trait MCPClientOps: Send {
    async fn initialize(&mut self) -> Result<(), RociError>;
    async fn list_tools(&mut self) -> Result<Vec<MCPToolSchema>, RociError>;
    async fn instructions(&mut self) -> Result<Option<String>, RociError>;
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

    async fn instructions(&mut self) -> Result<Option<String>, RociError> {
        MCPClient::instructions(self)
    }

    async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<MCPToolCallResult, RociError> {
        MCPClient::call_tool(self, name, arguments).await
    }
}
