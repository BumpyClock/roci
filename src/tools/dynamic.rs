//! Dynamic tool provider — runtime-discovered tools (e.g., MCP).

use async_trait::async_trait;

use super::arguments::ToolArguments;
use super::tool::ToolExecutionContext;
use super::types::AgentToolParameters;
use crate::error::RociError;

/// A tool discovered at runtime (e.g., from MCP server).
#[derive(Debug, Clone)]
pub struct DynamicTool {
    pub name: String,
    pub description: String,
    pub parameters: AgentToolParameters,
}

/// Trait for providers that can discover and execute tools at runtime.
#[async_trait]
pub trait DynamicToolProvider: Send + Sync {
    /// List available tools.
    async fn list_tools(&self) -> Result<Vec<DynamicTool>, RociError>;

    /// Execute a tool by name.
    async fn execute_tool(
        &self,
        name: &str,
        args: &ToolArguments,
        ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError>;
}
