//! Dynamic tool provider -- runtime-discovered tools (e.g., MCP).

use std::sync::Arc;

use async_trait::async_trait;

use super::arguments::ToolArguments;
use super::tool::{
    Tool, ToolExecutionContext, ToolPromptMetadata, ToolResultSizePolicy, ToolSafetyPlan,
    ToolSafetySummary,
};
use super::types::AgentToolParameters;
use crate::error::RociError;

/// A tool discovered at runtime (e.g., from MCP server).
#[derive(Debug, Clone)]
pub struct DynamicTool {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub prompt: Option<String>,
    pub prompt_metadata: ToolPromptMetadata,
    pub result_policy: ToolResultSizePolicy,
    pub parameters: AgentToolParameters,
    pub safety: ToolSafetyPlan,
    pub safety_summary: ToolSafetySummary,
}

impl DynamicTool {
    /// Create a dynamic tool with fail-closed safety metadata by default.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: AgentToolParameters,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            aliases: Vec::new(),
            prompt: None,
            prompt_metadata: ToolPromptMetadata::default(),
            result_policy: ToolResultSizePolicy::default(),
            parameters,
            safety: ToolSafetyPlan::default(),
            safety_summary: ToolSafetySummary::default(),
        }
    }

    /// Set explicit safety metadata for a dynamic tool.
    pub fn with_safety(mut self, plan: ToolSafetyPlan, summary: ToolSafetySummary) -> Self {
        self.safety = plan;
        self.safety_summary = summary;
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
    aliases: Vec<String>,
    prompt: Option<String>,
    prompt_metadata: ToolPromptMetadata,
    result_policy: ToolResultSizePolicy,
    parameters: AgentToolParameters,
    safety: ToolSafetyPlan,
    safety_summary: ToolSafetySummary,
}

impl DynamicToolAdapter {
    /// Create a new adapter for a discovered tool.
    pub fn new(provider: Arc<dyn DynamicToolProvider>, tool: DynamicTool) -> Self {
        Self {
            provider,
            name: tool.name,
            description: tool.description,
            aliases: tool.aliases,
            prompt: tool.prompt,
            prompt_metadata: tool.prompt_metadata,
            result_policy: tool.result_policy,
            parameters: tool.parameters,
            safety: tool.safety,
            safety_summary: tool.safety_summary,
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

    fn aliases(&self) -> &[String] {
        &self.aliases
    }

    fn prompt(&self) -> &str {
        self.prompt.as_deref().unwrap_or(&self.description)
    }

    fn prompt_metadata(&self) -> ToolPromptMetadata {
        self.prompt_metadata.clone()
    }

    fn result_policy(&self) -> ToolResultSizePolicy {
        self.result_policy
    }

    fn parameters(&self) -> &AgentToolParameters {
        &self.parameters
    }

    fn safety(&self, _args: &ToolArguments) -> ToolSafetyPlan {
        self.safety.clone()
    }

    fn safety_summary(&self) -> ToolSafetySummary {
        self.safety_summary
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
    use super::super::tool::ToolSafetyKind;
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
    fn dynamic_tool_defaults_to_fail_closed_safety() {
        let tool = DynamicTool::new("dynamic", "dynamic tool", AgentToolParameters::empty());

        assert_eq!(tool.safety, ToolSafetyPlan::default());
        assert_eq!(tool.safety_summary, ToolSafetySummary::default());
        assert_eq!(tool.safety.approval.kind, ToolSafetyKind::Other);
        assert!(!tool.safety.read_only);
        assert!(!tool.safety.destructive);
        assert!(!tool.safety.concurrency_safe);
    }

    #[test]
    fn dynamic_tool_adapter_uses_declared_safety_metadata() {
        let plan = ToolSafetyPlan::safe_read_only(ToolSafetyKind::Read);
        let summary = ToolSafetySummary {
            read_only_by_default: true,
            destructive_by_default: false,
            concurrency_safe_by_default: true,
            approval_kind: ToolSafetyKind::Read,
        };
        let adapter = DynamicToolAdapter::new(
            Arc::new(NoopProvider),
            DynamicTool::new("dynamic", "dynamic tool", AgentToolParameters::empty())
                .with_safety(plan.clone(), summary),
        );

        assert_eq!(
            adapter.safety(&ToolArguments::new(serde_json::json!({}))),
            plan
        );
        assert_eq!(adapter.safety_summary(), summary);
    }

    #[test]
    fn prompt_metadata_dynamic_tool_adapter_uses_declared_metadata() {
        let metadata = ToolPromptMetadata {
            guidelines: vec!["Use dynamic context carefully.".to_string()],
            search_hint: Some("future-search-only".to_string()),
        };
        let mut tool = DynamicTool::new("dynamic", "UI description", AgentToolParameters::empty());
        tool.prompt = Some("Model prompt".to_string());
        tool.prompt_metadata = metadata.clone();
        let adapter = DynamicToolAdapter::new(Arc::new(NoopProvider), tool);

        assert_eq!(adapter.description(), "UI description");
        assert_eq!(adapter.prompt(), "Model prompt");
        assert_eq!(adapter.prompt_metadata(), metadata);
    }
}
