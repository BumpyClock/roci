//! Dynamic tool provider — runtime-discovered tools (e.g., MCP).

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockProvider {
        last_call: Mutex<Option<String>>,
    }

    #[async_trait]
    impl DynamicToolProvider for MockProvider {
        async fn list_tools(&self) -> Result<Vec<DynamicTool>, RociError> {
            Ok(Vec::new())
        }

        async fn execute_tool(
            &self,
            name: &str,
            _args: &ToolArguments,
            _ctx: &ToolExecutionContext,
        ) -> Result<serde_json::Value, RociError> {
            let mut last_call = self.last_call.lock().expect("lock should succeed");
            *last_call = Some(name.to_string());
            Ok(serde_json::json!({ "ok": true }))
        }
    }

    #[tokio::test]
    async fn adapter_delegates_execute_to_provider() {
        let provider = Arc::new(MockProvider {
            last_call: Mutex::new(None),
        });
        let provider_dyn: Arc<dyn DynamicToolProvider> = provider.clone();
        let tool = DynamicTool {
            name: "dynamic".into(),
            description: "dynamic tool".into(),
            parameters: AgentToolParameters::empty(),
        };
        let adapter = DynamicToolAdapter::new(provider_dyn, tool);

        let result = adapter
            .execute(
                &ToolArguments::new(serde_json::json!({})),
                &ToolExecutionContext::default(),
            )
            .await
            .expect("execute should succeed");

        assert_eq!(result["ok"], true);
        let last_call = provider
            .last_call
            .lock()
            .expect("lock should succeed")
            .clone();
        assert_eq!(last_call.as_deref(), Some("dynamic"));
    }
}
