use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use serde_json::json;
use tokio::time::{sleep, timeout, Duration};

use crate::agent::runtime::AgentConfig;
use crate::agent::subagents::types::{
    DelegateSubagentRequest, DelegateSubagentResult, SendSubagentMessageResult, SubagentCaller,
    SubagentCancelResult, SubagentId, SubagentKnownChild, SubagentProfile, SubagentStatus,
    SubagentSupervisorConfig,
};
use crate::agent::subagents::{
    ModelCandidate, SubagentProfileRegistry, SubagentRoutingController, SubagentRoutingTools,
};
use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::capabilities::{ModelCapabilities, ModelInputCapabilities};
use crate::models::LanguageModel;
use crate::provider::{
    ModelProvider, ProviderFactory, ProviderRegistry, ProviderRequest, ProviderResponse,
};
use crate::tools::{Tool, ToolApproval, ToolArguments, ToolExecutionContext};
use crate::types::{StreamEventType, TextStreamDelta, Usage};

fn test_model() -> LanguageModel {
    LanguageModel::Known {
        provider_key: "test".into(),
        model_id: "test-model".into(),
    }
}

fn test_profile(name: &str, is_default: bool) -> SubagentProfile {
    SubagentProfile {
        name: name.into(),
        system_prompt: Some("You are a routed test sub-agent.".into()),
        models: vec![ModelCandidate {
            provider: "test".into(),
            model: "test-model".into(),
            reasoning_effort: None,
        }],
        default: is_default,
        ..Default::default()
    }
}

fn profile_registry(default_profile: Option<&str>) -> SubagentProfileRegistry {
    let mut profiles = SubagentProfileRegistry::new();
    profiles
        .register(test_profile(
            "test:alpha",
            default_profile == Some("test:alpha"),
        ))
        .expect("alpha profile should register");
    profiles
        .register(test_profile(
            "test:beta",
            default_profile == Some("test:beta"),
        ))
        .expect("beta profile should register");
    profiles
}

fn provider_registry(response_text: &str, completes: bool) -> Arc<ProviderRegistry> {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(TestProviderFactory {
        response_text: response_text.into(),
        completes,
    }));
    Arc::new(registry)
}

fn test_base_config() -> AgentConfig {
    AgentConfig {
        candidates: vec![test_model()],
        ..AgentConfig::default()
    }
}

fn controller(response_text: &str, completes: bool) -> SubagentRoutingController {
    SubagentRoutingController::new(
        provider_registry(response_text, completes),
        RociConfig::default(),
        test_base_config(),
        SubagentSupervisorConfig::default(),
        profile_registry(Some("test:alpha")),
    )
}

fn main_caller() -> SubagentCaller {
    SubagentCaller::main_agent()
}

fn child_caller() -> SubagentCaller {
    SubagentCaller::child(SubagentId::nil(), 1)
}

fn delegate_request(profile: Option<&str>, run_in_background: bool) -> DelegateSubagentRequest {
    DelegateSubagentRequest {
        profile: profile.map(str::to_string),
        task: "summarize routing state".into(),
        label: Some("routing-test".into()),
        run_in_background,
    }
}

fn assert_config_error_contains(error: RociError, expected: &str) {
    assert!(matches!(
        error,
        RociError::Configuration(message) if message.contains(expected)
    ));
}

fn routing_tools(controller: SubagentRoutingController) -> Vec<Arc<dyn Tool>> {
    SubagentRoutingTools::new(Arc::new(controller)).tools()
}

fn routing_tool<'a>(tools: &'a [Arc<dyn Tool>], name: &str) -> &'a Arc<dyn Tool> {
    tools
        .iter()
        .find(|tool| tool.name() == name)
        .unwrap_or_else(|| panic!("{name} tool should exist"))
}

async fn execute_tool(
    tool: &Arc<dyn Tool>,
    args: serde_json::Value,
) -> Result<serde_json::Value, RociError> {
    tool.execute(&ToolArguments::new(args), &ToolExecutionContext::default())
        .await
}

struct TestProviderFactory {
    response_text: String,
    completes: bool,
}

impl ProviderFactory for TestProviderFactory {
    fn provider_keys(&self) -> &[&str] {
        &["test"]
    }

    fn requires_credentials(&self, _provider_key: &str) -> bool {
        false
    }

    fn create(
        &self,
        _config: &RociConfig,
        provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        Ok(Box::new(TestProvider {
            provider_key: provider_key.into(),
            model_id: model_id.into(),
            response_text: self.response_text.clone(),
            completes: self.completes,
            capabilities: ModelCapabilities {
                supports_streaming: true,
                input: ModelInputCapabilities::default(),
                ..ModelCapabilities::default()
            },
        }))
    }
}

struct TestProvider {
    provider_key: String,
    model_id: String,
    response_text: String,
    completes: bool,
    capabilities: ModelCapabilities,
}

#[async_trait]
impl ModelProvider for TestProvider {
    fn provider_name(&self) -> &str {
        &self.provider_key
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        Err(RociError::UnsupportedOperation(
            "routing test provider uses stream_text".into(),
        ))
    }

    async fn stream_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        let text = self.response_text.clone();
        let text_delta = TextStreamDelta {
            text,
            event_type: StreamEventType::TextDelta,
            tool_call: None,
            finish_reason: None,
            usage: None,
            reasoning: None,
            reasoning_signature: None,
            reasoning_type: None,
        };
        if self.completes {
            let done = TextStreamDelta {
                text: String::new(),
                event_type: StreamEventType::Done,
                tool_call: None,
                finish_reason: None,
                usage: Some(Usage::default()),
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            };
            Ok(Box::pin(stream::iter(vec![Ok(text_delta), Ok(done)])))
        } else {
            Ok(Box::pin(
                stream::once(async move { Ok(text_delta) }).chain(stream::pending()),
            ))
        }
    }
}

#[tokio::test]
async fn subagent_routing_delegate_without_profile_uses_default_profile() {
    let controller = controller("default profile summary", true);

    let result = controller
        .delegate(delegate_request(None, false), &main_caller())
        .await
        .expect("delegate should use default profile");

    assert_eq!(result.profile_id, "test:alpha");
    assert_eq!(result.status, SubagentStatus::Completed);
}

#[tokio::test]
async fn subagent_routing_delegate_without_default_profile_returns_clear_error() {
    let controller = SubagentRoutingController::new(
        provider_registry("unused", true),
        RociConfig::default(),
        test_base_config(),
        SubagentSupervisorConfig::default(),
        profile_registry(None),
    );

    let error = controller
        .delegate(delegate_request(None, false), &main_caller())
        .await
        .expect_err("missing default should fail");

    assert_config_error_contains(error, "no default subagent profile configured");
}

#[tokio::test]
async fn subagent_routing_delegate_unknown_profile_returns_clear_error() {
    let controller = controller("unused", true);

    let error = controller
        .delegate(
            delegate_request(Some("test:missing"), false),
            &main_caller(),
        )
        .await
        .expect_err("unknown profile should fail");

    assert_config_error_contains(error, "unknown subagent profile 'test:missing'");
}

#[tokio::test]
async fn subagent_routing_list_profiles_returns_sorted_profile_summaries() {
    let controller = controller("unused", true);

    let summaries = controller
        .list_profiles(&main_caller())
        .expect("profile listing should succeed");

    let names = summaries
        .into_iter()
        .map(|summary| summary.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["test:alpha", "test:beta"]);
}

#[tokio::test]
async fn subagent_routing_foreground_delegate_returns_compact_summary() {
    let controller = controller("last assistant summary", true);

    let result = controller
        .delegate(delegate_request(Some("test:beta"), false), &main_caller())
        .await
        .expect("foreground delegate should complete");

    assert_eq!(result.profile_id, "test:beta");
    assert_eq!(result.status, SubagentStatus::Completed);
    assert_eq!(result.summary, "last assistant summary");
    assert!(result.artifacts.is_empty());
    assert!(result.child_thread_id.is_some());
    assert!(result.usage.is_none());
    assert!(result.error.is_none());
}

#[tokio::test]
async fn subagent_routing_background_delegate_is_listed_until_waited() {
    let controller = controller("background summary", true);

    let delegated = controller
        .delegate(delegate_request(Some("test:beta"), true), &main_caller())
        .await
        .expect("background delegate should start");
    assert_eq!(delegated.status, SubagentStatus::Running);
    assert!(delegated.child_thread_id.is_some());

    let listed = controller
        .list_subagents(&main_caller())
        .await
        .expect("list should succeed");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].subagent_id, delegated.subagent_id);
    assert_eq!(listed[0].profile_id, "test:beta");

    let waited = controller
        .wait_subagent(delegated.subagent_id, &main_caller())
        .await
        .expect("wait should complete");
    assert_eq!(waited.status, SubagentStatus::Completed);
    assert_eq!(waited.child_thread_id, delegated.child_thread_id);

    let listed_after_wait = controller
        .list_subagents(&main_caller())
        .await
        .expect("list after wait should succeed");
    assert!(listed_after_wait.is_empty());
}

#[tokio::test]
async fn subagent_routing_wait_subagent_caches_completion_result() {
    let controller = controller("cached summary", true);
    let delegated = controller
        .delegate(delegate_request(None, true), &main_caller())
        .await
        .expect("background delegate should start");

    let first = controller
        .wait_subagent(delegated.subagent_id, &main_caller())
        .await
        .expect("first wait should complete");
    let second = controller
        .wait_subagent(delegated.subagent_id, &main_caller())
        .await
        .expect("second wait should return cache");

    assert_eq!(first, second);
    assert_eq!(second.summary, "cached summary");
}

#[tokio::test]
async fn subagent_routing_wait_subagent_concurrent_callers_share_completion_result() {
    let controller = controller("shared wait summary", true);
    let delegated = controller
        .delegate(delegate_request(None, true), &main_caller())
        .await
        .expect("background delegate should start");

    let caller = main_caller();
    let (first, second) = tokio::join!(
        controller.wait_subagent(delegated.subagent_id, &caller),
        controller.wait_subagent(delegated.subagent_id, &caller),
    );
    let first = first.expect("first wait should complete");
    let second = second.expect("second wait should share completion");

    assert_eq!(first, second);
    assert_eq!(first.status, SubagentStatus::Completed);
    assert_eq!(first.summary, "shared wait summary");
}

#[tokio::test]
async fn subagent_routing_cancel_subagent_reports_canceled() {
    let controller = controller("partial before cancel", false);
    let delegated = controller
        .delegate(delegate_request(None, true), &main_caller())
        .await
        .expect("background delegate should start");

    let cancel = timeout(
        Duration::from_secs(2),
        controller.cancel_subagent(delegated.subagent_id, &main_caller()),
    )
    .await
    .expect("cancel should not hang")
    .expect("cancel should succeed");

    assert_eq!(cancel.subagent_id, delegated.subagent_id);
    assert!(cancel.canceled);
    assert_eq!(cancel.status, SubagentStatus::Aborted);
}

#[tokio::test]
async fn subagent_routing_cancel_completed_unwaited_child_reports_not_canceled() {
    let controller = controller("completed before cancel", true);
    let delegated = controller
        .delegate(delegate_request(None, true), &main_caller())
        .await
        .expect("background delegate should start");

    timeout(Duration::from_secs(2), async {
        loop {
            let listed = controller
                .list_subagents(&main_caller())
                .await
                .expect("list should succeed");
            if listed.iter().any(|child| {
                child.subagent_id == delegated.subagent_id
                    && child.status == SubagentStatus::Completed
            }) {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("child should complete before cancel");

    let cancel = controller
        .cancel_subagent(delegated.subagent_id, &main_caller())
        .await
        .expect("cancel should report terminal child");

    assert_eq!(cancel.subagent_id, delegated.subagent_id);
    assert!(!cancel.canceled);
    assert_eq!(cancel.status, SubagentStatus::Completed);

    let waited = controller
        .wait_subagent(delegated.subagent_id, &main_caller())
        .await
        .expect("wait after cancel should return cached completion");
    assert_eq!(waited.status, SubagentStatus::Completed);
    assert_eq!(waited.summary, "completed before cancel");
}

#[tokio::test]
async fn subagent_routing_management_methods_reject_child_callers() {
    let controller = controller("unused", true);
    let caller = child_caller();

    assert_config_error_contains(
        controller
            .list_profiles(&caller)
            .expect_err("child list profiles should fail"),
        "only available to the main agent",
    );
    assert_config_error_contains(
        controller
            .delegate(delegate_request(None, false), &caller)
            .await
            .expect_err("child delegate should fail"),
        "only available to the main agent",
    );
    assert_config_error_contains(
        controller
            .list_subagents(&caller)
            .await
            .expect_err("child list subagents should fail"),
        "only available to the main agent",
    );
    assert_config_error_contains(
        controller
            .wait_subagent(SubagentId::nil(), &caller)
            .await
            .expect_err("child wait should fail"),
        "only available to the main agent",
    );
    assert_config_error_contains(
        controller
            .cancel_subagent(SubagentId::nil(), &caller)
            .await
            .expect_err("child cancel should fail"),
        "only available to the main agent",
    );
    assert_config_error_contains(
        controller
            .send_subagent_message(SubagentId::nil(), "message", &caller)
            .await
            .expect_err("child send should fail"),
        "only available to the main agent",
    );
}

#[tokio::test]
async fn subagent_routing_send_subagent_message_accepts_active_child() {
    let controller = controller("active child", false);
    let delegated = controller
        .delegate(delegate_request(None, true), &main_caller())
        .await
        .expect("background delegate should start");

    let sent = controller
        .send_subagent_message(delegated.subagent_id, "parent note", &main_caller())
        .await
        .expect("active child should accept parent message");

    assert_eq!(sent.subagent_id, delegated.subagent_id);
    assert!(sent.accepted);

    controller
        .cancel_subagent(delegated.subagent_id, &main_caller())
        .await
        .expect("cleanup cancel should succeed");
}

#[tokio::test]
async fn subagent_routing_send_subagent_message_rejects_unknown_and_terminal_children() {
    let controller = controller("terminal summary", true);

    let unknown = controller
        .send_subagent_message(SubagentId::nil(), "message", &main_caller())
        .await
        .expect_err("unknown child should fail");
    assert_config_error_contains(
        unknown,
        "subagent 00000000-0000-0000-0000-000000000000 not found",
    );

    let delegated = controller
        .delegate(delegate_request(None, true), &main_caller())
        .await
        .expect("background delegate should start");
    controller
        .wait_subagent(delegated.subagent_id, &main_caller())
        .await
        .expect("wait should complete child");

    let terminal = controller
        .send_subagent_message(delegated.subagent_id, "message", &main_caller())
        .await
        .expect_err("terminal child should fail");
    assert_config_error_contains(terminal, "cannot send message to terminal subagent");
}

#[tokio::test]
async fn subagent_routing_tools_delegate_subagent_tool_executes_foreground_and_returns_json() {
    let tools = routing_tools(controller("tool foreground summary", true));
    let tool = routing_tool(&tools, "delegate_subagent");

    assert_eq!(tool.approval(), ToolApproval::safe_host_input());
    assert_eq!(
        tool.parameters().schema["required"],
        serde_json::Value::Array(vec![serde_json::Value::String("task".into())])
    );

    let value = execute_tool(
        tool,
        json!({
            "profile": "test:beta",
            "task": "summarize routing state",
            "label": "tool-foreground",
            "run_in_background": false
        }),
    )
    .await
    .expect("delegate tool should execute");
    let result: DelegateSubagentResult =
        serde_json::from_value(value).expect("delegate tool output should be DTO JSON");

    assert_eq!(result.profile_id, "test:beta");
    assert_eq!(result.status, SubagentStatus::Completed);
    assert_eq!(result.summary, "tool foreground summary");
    assert!(result.artifacts.is_empty());
}

#[tokio::test]
async fn subagent_routing_tools_delegate_subagent_tool_accepts_minimal_task_args() {
    let tools = routing_tools(controller("minimal task summary", true));
    let tool = routing_tool(&tools, "delegate_subagent");

    let value = execute_tool(tool, json!({"task": "minimal task"}))
        .await
        .expect("minimal delegate args should execute");
    let result: DelegateSubagentResult =
        serde_json::from_value(value).expect("delegate tool output should be DTO JSON");

    assert_eq!(result.profile_id, "test:alpha");
    assert_eq!(result.status, SubagentStatus::Completed);
    assert_eq!(result.summary, "minimal task summary");
}

#[tokio::test]
async fn subagent_routing_tools_list_subagents_tool_returns_known_children() {
    let tools = routing_tools(controller("known child summary", true));
    let delegate_tool = routing_tool(&tools, "delegate_subagent");
    let list_tool = routing_tool(&tools, "list_subagents");

    let delegated: DelegateSubagentResult = serde_json::from_value(
        execute_tool(
            delegate_tool,
            json!({
                "profile": "test:beta",
                "task": "start background child",
                "label": "known-child",
                "run_in_background": true
            }),
        )
        .await
        .expect("background delegate should execute"),
    )
    .expect("delegate output should be DTO JSON");

    let value = execute_tool(list_tool, json!({}))
        .await
        .expect("list tool should execute");
    let children: Vec<SubagentKnownChild> =
        serde_json::from_value(value).expect("list tool output should be child JSON");

    assert_eq!(children.len(), 1);
    assert_eq!(children[0].subagent_id, delegated.subagent_id);
    assert_eq!(children[0].profile_id, "test:beta");
    assert_eq!(children[0].label.as_deref(), Some("known-child"));
}

#[tokio::test]
async fn subagent_routing_tools_wait_subagent_tool_returns_compact_result() {
    let tools = routing_tools(controller("waited compact summary", true));
    let delegate_tool = routing_tool(&tools, "delegate_subagent");
    let wait_tool = routing_tool(&tools, "wait_subagent");

    let delegated: DelegateSubagentResult = serde_json::from_value(
        execute_tool(
            delegate_tool,
            json!({
                "task": "wait for background child",
                "run_in_background": true
            }),
        )
        .await
        .expect("background delegate should execute"),
    )
    .expect("delegate output should be DTO JSON");

    let value = execute_tool(wait_tool, json!({"subagent_id": delegated.subagent_id}))
        .await
        .expect("wait tool should execute");
    let result: DelegateSubagentResult =
        serde_json::from_value(value).expect("wait tool output should be DTO JSON");

    assert_eq!(result.subagent_id, delegated.subagent_id);
    assert_eq!(result.status, SubagentStatus::Completed);
    assert_eq!(result.summary, "waited compact summary");
}

#[tokio::test]
async fn subagent_routing_tools_cancel_subagent_tool_returns_cancel_result() {
    let tools = routing_tools(controller("cancel partial summary", false));
    let delegate_tool = routing_tool(&tools, "delegate_subagent");
    let cancel_tool = routing_tool(&tools, "cancel_subagent");

    let delegated: DelegateSubagentResult = serde_json::from_value(
        execute_tool(
            delegate_tool,
            json!({
                "task": "start cancellable child",
                "run_in_background": true
            }),
        )
        .await
        .expect("background delegate should execute"),
    )
    .expect("delegate output should be DTO JSON");

    let value = timeout(
        Duration::from_secs(2),
        execute_tool(cancel_tool, json!({"subagent_id": delegated.subagent_id})),
    )
    .await
    .expect("cancel tool should not hang")
    .expect("cancel tool should execute");
    let result: SubagentCancelResult =
        serde_json::from_value(value).expect("cancel tool output should be DTO JSON");

    assert_eq!(result.subagent_id, delegated.subagent_id);
    assert!(result.canceled);
    assert_eq!(result.status, SubagentStatus::Aborted);
}

#[tokio::test]
async fn subagent_routing_tools_send_subagent_message_tool_returns_acceptance_or_error() {
    let tools = routing_tools(controller("active child summary", false));
    let delegate_tool = routing_tool(&tools, "delegate_subagent");
    let send_tool = routing_tool(&tools, "send_subagent_message");
    let cancel_tool = routing_tool(&tools, "cancel_subagent");

    let delegated: DelegateSubagentResult = serde_json::from_value(
        execute_tool(
            delegate_tool,
            json!({
                "task": "start messageable child",
                "run_in_background": true
            }),
        )
        .await
        .expect("background delegate should execute"),
    )
    .expect("delegate output should be DTO JSON");

    let value = execute_tool(
        send_tool,
        json!({
            "subagent_id": delegated.subagent_id,
            "message": "parent steering note"
        }),
    )
    .await
    .expect("send tool should execute");
    let result: SendSubagentMessageResult =
        serde_json::from_value(value).expect("send tool output should be DTO JSON");

    assert_eq!(result.subagent_id, delegated.subagent_id);
    assert!(result.accepted);

    execute_tool(cancel_tool, json!({"subagent_id": delegated.subagent_id}))
        .await
        .expect("cleanup cancel should execute");

    let error = execute_tool(
        send_tool,
        json!({"subagent_id": SubagentId::nil(), "message": "x"}),
    )
    .await
    .expect_err("unknown child should return error");
    assert_config_error_contains(
        error,
        "subagent 00000000-0000-0000-0000-000000000000 not found",
    );
}
