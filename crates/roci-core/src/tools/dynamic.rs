//! Dynamic tool provider -- runtime-discovered tools (e.g., MCP).

use std::sync::Arc;

use async_trait::async_trait;

use super::arguments::ToolArguments;
use super::tool::{Tool, ToolExecutionContext};
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

/// Adapter that exposes a [`DynamicTool`] through the core [`Tool`] trait.
pub struct DynamicToolAdapter {
    provider: Arc<dyn DynamicToolProvider>,
    name: String,
    description: String,
    parameters: AgentToolParameters,
}

impl DynamicToolAdapter {
    /// Create a new adapter for a discovered tool.
    pub fn new(provider: Arc<dyn DynamicToolProvider>, tool: DynamicTool) -> Self {
        Self {
            provider,
            name: tool.name,
            description: tool.description,
            parameters: tool.parameters,
        }
    }
}

#[async_trait]
impl Tool for DynamicToolAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> &AgentToolParameters {
        &self.parameters
    }

    async fn execute(
        &self,
        args: &ToolArguments,
        ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError> {
        self.provider.execute_tool(&self.name, args, ctx).await
    }
}
