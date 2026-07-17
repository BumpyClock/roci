//! Tool wrappers for sub-agent routing controller.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use crate::tools::{
    AgentTool, AgentToolParameters, Tool, ToolArguments, ToolExecutionContext, ToolSafetyKind,
    ToolSafetyPlan, ToolSafetySummary,
};

use super::routing::SubagentRoutingController;
use super::types::{DelegateSubagentRequest, SubagentCaller, SubagentId};

/// Model-callable tools backed by a [`SubagentRoutingController`].
#[derive(Clone)]
pub struct SubagentRoutingTools {
    controller: Arc<SubagentRoutingController>,
}

#[derive(Debug, Deserialize)]
struct SubagentIdToolArgs {
    subagent_id: SubagentId,
}

#[derive(Debug, Deserialize)]
struct EmptyToolArgs {}

#[derive(Debug, Deserialize)]
struct SendSubagentMessageToolArgs {
    subagent_id: SubagentId,
    message: String,
}

impl SubagentRoutingTools {
    /// Create sub-agent routing tools for one controller.
    pub fn new(controller: Arc<SubagentRoutingController>) -> Self {
        Self { controller }
    }

    /// Build all model-callable sub-agent routing tools.
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![
            self.delegate_subagent_tool(),
            self.list_subagents_tool(),
            self.wait_subagent_tool(),
            self.cancel_subagent_tool(),
            self.send_subagent_message_tool(),
        ]
    }

    fn delegate_subagent_tool(&self) -> Arc<dyn Tool> {
        let profile_names = self
            .controller
            .list_profiles(&SubagentCaller::main_agent())
            .unwrap_or_default()
            .into_iter()
            .map(|profile| profile.name)
            .collect::<Vec<_>>();
        let profile_schema = if profile_names.is_empty() {
            json!({
                "type": "string",
                "description": "Optional sub-agent profile; omit to use the configured default"
            })
        } else {
            json!({
                "type": "string",
                "description": "Optional sub-agent profile; omit to use the configured default",
                "enum": profile_names
            })
        };
        let controller = self.controller.clone();
        Arc::new(
            AgentTool::new(
                "delegate_subagent",
                "Delegate a task to a sub-agent",
                AgentToolParameters::from_schema(json!({
                    "type": "object",
                    "properties": {
                        "profile": profile_schema,
                        "task": { "type": "string" },
                        "label": { "type": "string" },
                        "run_in_background": { "type": "boolean" }
                    },
                    "required": ["task"]
                })),
                move |args: ToolArguments, ctx: ToolExecutionContext| {
                    let controller = controller.clone();
                    async move {
                        let request: DelegateSubagentRequest = args.deserialize()?;
                        let result = controller
                            .delegate_from_tool(
                                request,
                                &SubagentCaller::main_agent(),
                                ctx.tool_call_id.clone(),
                            )
                            .await?;
                        Ok(serde_json::to_value(result)?)
                    }
                },
            )
            .with_static_safety(ToolSafetyPlan::host_input(), host_input_safety_summary()),
        )
    }

    fn list_subagents_tool(&self) -> Arc<dyn Tool> {
        let controller = self.controller.clone();
        Arc::new(
            AgentTool::new(
                "list_subagents",
                "List child sub-agents known to the current session",
                AgentToolParameters::from_schema(json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                })),
                move |args: ToolArguments, _ctx: ToolExecutionContext| {
                    let controller = controller.clone();
                    async move {
                        let _: EmptyToolArgs = args.deserialize()?;
                        let children = controller
                            .list_subagents(&SubagentCaller::main_agent())
                            .await?;
                        Ok(serde_json::to_value(children)?)
                    }
                },
            )
            .with_static_safety(ToolSafetyPlan::host_input(), host_input_safety_summary()),
        )
    }

    fn wait_subagent_tool(&self) -> Arc<dyn Tool> {
        let controller = self.controller.clone();
        Arc::new(
            AgentTool::new(
                "wait_subagent",
                "Wait for a child sub-agent and return its compact result",
                subagent_id_schema(),
                move |args: ToolArguments, _ctx: ToolExecutionContext| {
                    let controller = controller.clone();
                    async move {
                        let args: SubagentIdToolArgs = args.deserialize()?;
                        let result = controller
                            .wait_subagent(args.subagent_id, &SubagentCaller::main_agent())
                            .await?;
                        Ok(serde_json::to_value(result)?)
                    }
                },
            )
            .with_static_safety(ToolSafetyPlan::host_input(), host_input_safety_summary()),
        )
    }

    fn cancel_subagent_tool(&self) -> Arc<dyn Tool> {
        let controller = self.controller.clone();
        Arc::new(
            AgentTool::new(
                "cancel_subagent",
                "Cancel an active child sub-agent",
                subagent_id_schema(),
                move |args: ToolArguments, _ctx: ToolExecutionContext| {
                    let controller = controller.clone();
                    async move {
                        let args: SubagentIdToolArgs = args.deserialize()?;
                        let result = controller
                            .cancel_subagent(args.subagent_id, &SubagentCaller::main_agent())
                            .await?;
                        Ok(serde_json::to_value(result)?)
                    }
                },
            )
            .with_static_safety(ToolSafetyPlan::host_input(), host_input_safety_summary()),
        )
    }

    fn send_subagent_message_tool(&self) -> Arc<dyn Tool> {
        let controller = self.controller.clone();
        Arc::new(
            AgentTool::new(
                "send_subagent_message",
                "Send a steering message to an active child sub-agent",
                AgentToolParameters::from_schema(json!({
                    "type": "object",
                    "properties": {
                        "subagent_id": { "type": "string" },
                        "message": { "type": "string" }
                    },
                    "required": ["subagent_id", "message"]
                })),
                move |args: ToolArguments, _ctx: ToolExecutionContext| {
                    let controller = controller.clone();
                    async move {
                        let args: SendSubagentMessageToolArgs = args.deserialize()?;
                        let result = controller
                            .send_subagent_message(
                                args.subagent_id,
                                args.message,
                                &SubagentCaller::main_agent(),
                            )
                            .await?;
                        Ok(serde_json::to_value(result)?)
                    }
                },
            )
            .with_static_safety(ToolSafetyPlan::host_input(), host_input_safety_summary()),
        )
    }
}

fn host_input_safety_summary() -> ToolSafetySummary {
    ToolSafetySummary {
        read_only_by_default: false,
        destructive_by_default: false,
        concurrency_safe_by_default: false,
        approval_kind: ToolSafetyKind::Other,
    }
}

fn subagent_id_schema() -> AgentToolParameters {
    AgentToolParameters::from_schema(json!({
        "type": "object",
        "properties": {
            "subagent_id": { "type": "string" }
        },
        "required": ["subagent_id"]
    }))
}
