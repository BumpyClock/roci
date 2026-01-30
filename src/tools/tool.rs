//! Tool trait and closure-based tool wrapper.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;

use super::arguments::ToolArguments;
use super::types::AgentToolParameters;
use crate::error::RociError;

/// Context available during tool execution.
#[derive(Debug, Clone, Default)]
pub struct ToolExecutionContext {
    /// Additional metadata for the tool.
    pub metadata: serde_json::Value,
}

/// Core tool trait — implement to create custom tools.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (must match what the model calls).
    fn name(&self) -> &str;

    /// Human-readable description.
    fn description(&self) -> &str;

    /// JSON Schema parameters.
    fn parameters(&self) -> &AgentToolParameters;

    /// Execute the tool with parsed arguments.
    async fn execute(
        &self,
        args: &ToolArguments,
        ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError>;
}

/// Type alias for the tool handler function.
type ToolHandler = dyn Fn(
        ToolArguments,
        ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, RociError>> + Send>>
    + Send
    + Sync;

/// Closure-based tool for quick tool creation.
pub struct AgentTool {
    name: String,
    description: String,
    parameters: AgentToolParameters,
    handler: Arc<ToolHandler>,
}

impl AgentTool {
    /// Create a tool from a closure.
    pub fn new<F, Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: AgentToolParameters,
        handler: F,
    ) -> Self
    where
        F: Fn(ToolArguments, ToolExecutionContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value, RociError>> + Send + 'static,
    {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            handler: Arc::new(move |args, ctx| Box::pin(handler(args, ctx))),
        }
    }
}

#[async_trait]
impl Tool for AgentTool {
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
        (self.handler)(args.clone(), ctx.clone()).await
    }
}

impl std::fmt::Debug for AgentTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTool")
            .field("name", &self.name)
            .field("description", &self.description)
            .finish()
    }
}
