//! Dynamic tool provider -- runtime-discovered tools (e.g., MCP).

use std::sync::Arc;

use async_trait::async_trait;

use super::arguments::ToolArguments;
use super::tool::{Tool, ToolApproval, ToolExecutionContext};
use super::types::AgentToolParameters;
use crate::error::RociError;

/// A tool discovered at runtime (e.g., from MCP server).
#[derive(Debug, Clone)]
pub struct DynamicTool {
    pub name: String,
    pub description: String,
    pub parameters: AgentToolParameters,
    pub approval: ToolApproval,
}

impl DynamicTool {
    /// Create a dynamic tool with approval required by default.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: AgentToolParameters,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            approval: ToolApproval::requires_approval(super::tool::ToolApprovalKind::Other),
        }
    }

    /// Set explicit approval metadata for a dynamic tool.
    pub fn with_approval(mut self, approval: ToolApproval) -> Self {
        self.approval = approval;
        self
    }
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
    approval: ToolApproval,
}

impl DynamicToolAdapter {
    /// Create a new adapter for a discovered tool.
    pub fn new(provider: Arc<dyn DynamicToolProvider>, tool: DynamicTool) -> Self {
        Self {
            provider,
            name: tool.name,
            description: tool.description,
            parameters: tool.parameters,
            approval: tool.approval,
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

    fn approval(&self) -> ToolApproval {
        self.approval
    }

    async fn execute(
        &self,
        args: &ToolArguments,
        ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError> {
        self.provider.execute_tool(&self.name, args, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::super::tool::ToolApprovalKind;
    use super::*;

    struct NoopProvider;

    #[async_trait]
    impl DynamicToolProvider for NoopProvider {
        async fn list_tools(&self) -> Result<Vec<DynamicTool>, RociError> {
            Ok(Vec::new())
        }

        async fn execute_tool(
            &self,
            _name: &str,
            _args: &ToolArguments,
            _ctx: &ToolExecutionContext,
        ) -> Result<serde_json::Value, RociError> {
            Ok(serde_json::json!({}))
        }
    }

    #[test]
    fn dynamic_tool_defaults_to_approval_required() {
        let tool = DynamicTool::new("dynamic", "dynamic tool", AgentToolParameters::empty());

        assert_eq!(
            tool.approval,
            ToolApproval::requires_approval(ToolApprovalKind::Other)
        );
    }

    #[test]
    fn dynamic_tool_adapter_uses_declared_approval_metadata() {
        let adapter = DynamicToolAdapter::new(
            Arc::new(NoopProvider),
            DynamicTool::new("dynamic", "dynamic tool", AgentToolParameters::empty())
                .with_approval(ToolApproval::safe_read_only()),
        );

        assert_eq!(adapter.approval(), ToolApproval::safe_read_only());
    }
}
