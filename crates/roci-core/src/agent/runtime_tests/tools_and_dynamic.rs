use super::support::*;
use super::*;
#[cfg(feature = "agent")]
use crate::agent::runtime::config::AgentSubagentConfig;
#[cfg(feature = "agent")]
use crate::agent::subagents::{
    ModelCandidate, SubagentProfile, SubagentProfileRegistry, SubagentSupervisorConfig,
};
use crate::tools::dynamic::{DynamicTool, DynamicToolProvider};
use crate::tools::{
    AgentToolParameters, ToolSafetyKind, ToolSafetyPlan, ToolSafetySummary, ToolVisibilityPolicy,
};
#[cfg(feature = "agent")]
use crate::tools::{ToolArguments, ToolExecutionContext};
use std::sync::Arc;

#[tokio::test]
async fn set_tools_rejects_when_running() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    *agent.state.lock().await = AgentState::Running;

    let err = agent.set_tools(vec![dummy_tool("t1")]).await.unwrap_err();
    assert!(matches!(err, RociError::InvalidState(_)));
}

#[tokio::test]
async fn set_tools_replaces_runtime_tool_registry() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    agent
        .set_tools(vec![dummy_tool("t1"), dummy_tool("t2")])
        .await
        .unwrap();

    let names: Vec<String> = agent
        .tools
        .lock()
        .await
        .iter()
        .map(|tool| tool.name().to_string())
        .collect();
    assert_eq!(names, vec!["t1".to_string(), "t2".to_string()]);
}

#[tokio::test]
async fn resolve_tools_for_run_merges_static_and_dynamic_tools() {
    let provider: Arc<dyn DynamicToolProvider> =
        Arc::new(MockDynamicToolProvider::new(vec![DynamicTool::new(
            "dynamic",
            "dynamic tool",
            AgentToolParameters::empty(),
        )
        .with_safety(
            ToolSafetyPlan::approval_required(ToolSafetyKind::Other),
            ToolSafetySummary::default(),
        )]));

    let mut config = test_agent_config();
    config.tools = vec![dummy_tool("static")];
    config.dynamic_tool_providers = vec![Arc::clone(&provider)];

    let agent = AgentRuntime::new(test_registry(), test_config(), config);

    let tools = agent
        .resolve_tools_for_run()
        .await
        .expect("tools should resolve");
    let names = tools
        .iter()
        .map(|tool| tool.name().to_string())
        .collect::<Vec<_>>();

    assert!(names.contains(&"static".to_string()));
    assert!(names.contains(&"dynamic".to_string()));
}

#[tokio::test]
async fn resolve_tools_for_run_applies_visibility_policy_to_static_and_dynamic_tools() {
    let provider: Arc<dyn DynamicToolProvider> =
        Arc::new(MockDynamicToolProvider::new(vec![DynamicTool::new(
            "dynamic",
            "dynamic tool",
            AgentToolParameters::empty(),
        )
        .with_safety(
            ToolSafetyPlan::approval_required(ToolSafetyKind::Other),
            ToolSafetySummary::default(),
        )]));

    let mut config = test_agent_config();
    config.tools = vec![dummy_tool("static")];
    config.dynamic_tool_providers = vec![Arc::clone(&provider)];
    config.tool_visibility_policy = ToolVisibilityPolicy::allow_only(["dynamic"]);

    let agent = AgentRuntime::new(test_registry(), test_config(), config);

    let tools = agent
        .resolve_tools_for_run()
        .await
        .expect("tools should resolve");
    let names = tools
        .iter()
        .map(|tool| tool.name().to_string())
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["dynamic".to_string()]);
}

#[cfg(feature = "agent")]
#[tokio::test]
async fn subagent_runtime_wiring_resolve_tools_injects_routing_tools_when_enabled() {
    let mut config = test_agent_config();
    config.subagents = Some(test_subagent_config(true, None));
    config.api_key_override = Some("test-key".into());

    let agent = AgentRuntime::new(test_registry(), test_config(), config);

    let names = resolved_tool_names(&agent).await;

    assert!(names.contains(&"delegate_subagent".to_string()));
    assert!(names.contains(&"list_subagents".to_string()));
    assert!(names.contains(&"wait_subagent".to_string()));
    assert!(names.contains(&"cancel_subagent".to_string()));
    assert!(names.contains(&"send_subagent_message".to_string()));
}

#[cfg(feature = "agent")]
#[tokio::test]
async fn subagent_runtime_wiring_resolve_tools_hides_routing_tools_when_disabled() {
    let mut config = test_agent_config();
    config.subagents = Some(test_subagent_config(false, None));

    let agent = AgentRuntime::new(test_registry(), test_config(), config);

    let names = resolved_tool_names(&agent).await;

    assert!(!names.contains(&"delegate_subagent".to_string()));
    assert!(!names.contains(&"list_subagents".to_string()));
}

#[cfg(feature = "agent")]
#[tokio::test]
async fn subagent_runtime_wiring_default_profile_projects_parent_tool_visibility() {
    let mut config = test_agent_config();
    config.tools = vec![dummy_tool("safe_tool"), dummy_tool("danger_tool")];
    config.tool_visibility_policy = ToolVisibilityPolicy::allow_only([
        "safe_tool",
        "danger_tool",
        "delegate_subagent",
        "list_subagents",
    ]);
    config.subagents = Some(test_subagent_config(true, Some(vec!["danger_tool"])));

    let agent = AgentRuntime::new(test_registry(), test_config(), config);

    let names = resolved_tool_names(&agent).await;

    assert!(names.contains(&"safe_tool".to_string()));
    assert!(names.contains(&"delegate_subagent".to_string()));
    assert!(names.contains(&"list_subagents".to_string()));
    assert!(!names.contains(&"danger_tool".to_string()));
    assert!(!names.contains(&"wait_subagent".to_string()));
}

#[cfg(feature = "agent")]
#[tokio::test]
async fn subagent_runtime_wiring_delegate_tool_publishes_semantic_started_event() {
    let mut config = test_agent_config();
    config.subagents = Some(test_subagent_config(true, None));
    config.api_key_override = Some("test-key".into());
    let agent = AgentRuntime::new(
        registry_with_streaming_provider("test", 2, 3),
        test_config(),
        config,
    );
    agent.ensure_runtime_event_publisher().await;
    let mut events = agent.subscribe(None).await;
    tokio::task::yield_now().await;
    let delegate_tool = agent
        .resolve_tools_for_run()
        .await
        .expect("tools should resolve")
        .into_iter()
        .find(|tool| tool.name() == "delegate_subagent")
        .expect("delegate tool should be injected");

    let result = delegate_tool
        .execute(
            &ToolArguments::new(serde_json::json!({
                "task": "summarize runtime wiring",
                "run_in_background": true
            })),
            &ToolExecutionContext {
                tool_call_id: Some("parent-call-1".into()),
                tool_name: Some("delegate_subagent".into()),
                ..ToolExecutionContext::default()
            },
        )
        .await
        .expect("delegate tool should execute");

    assert_eq!(result["status"], "running");
    for attempt in 0..10 {
        // Other semantic events may arrive before the child lifecycle event.
        let event = tokio::time::timeout(std::time::Duration::from_millis(500), events.recv())
            .await
            .unwrap_or_else(|_| panic!("semantic subagent event timed out after attempt {attempt}"))
            .expect("runtime event should decode");
        if let AgentRuntimeEventPayload::SubagentStarted { subagent } = event.payload {
            assert_eq!(subagent.profile_id, "test:default");
            assert_eq!(
                subagent.parent_tool_call_id.as_deref(),
                Some("parent-call-1")
            );
            assert!(subagent.child_thread_id.is_some());
            return;
        }
    }
    panic!("expected subagent started event");
}

#[tokio::test]
async fn set_dynamic_tool_providers_replaces_runtime_registry() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    let provider: Arc<dyn DynamicToolProvider> = Arc::new(MockDynamicToolProvider::new(Vec::new()));

    agent
        .set_dynamic_tool_providers(vec![Arc::clone(&provider)])
        .await
        .expect("dynamic providers should be replaced");

    let providers = agent.dynamic_tool_providers.lock().await;
    assert_eq!(providers.len(), 1);
}

#[tokio::test]
async fn clear_dynamic_tool_providers_empties_registry() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());
    let provider: Arc<dyn DynamicToolProvider> = Arc::new(MockDynamicToolProvider::new(Vec::new()));

    agent
        .set_dynamic_tool_providers(vec![Arc::clone(&provider)])
        .await
        .expect("dynamic providers should be set");

    agent
        .clear_dynamic_tool_providers()
        .await
        .expect("dynamic providers should be cleared");

    let providers = agent.dynamic_tool_providers.lock().await;
    assert!(providers.is_empty());
}

#[cfg(feature = "agent")]
fn test_subagent_config(
    enabled: bool,
    default_agent_excluded_tools: Option<Vec<&str>>,
) -> AgentSubagentConfig {
    let mut profiles = SubagentProfileRegistry::new();
    profiles
        .register(SubagentProfile {
            name: "test:default".into(),
            models: vec![ModelCandidate {
                provider: "test".into(),
                model: "test-model".into(),
                reasoning_effort: None,
            }],
            default_agent_excluded_tools: default_agent_excluded_tools
                .unwrap_or_default()
                .into_iter()
                .map(str::to_string)
                .collect(),
            default: true,
            ..SubagentProfile::default()
        })
        .expect("default profile should register");
    AgentSubagentConfig {
        profiles,
        supervisor: SubagentSupervisorConfig::default(),
        enabled,
        main_profile: None,
    }
}

#[cfg(feature = "agent")]
async fn resolved_tool_names(agent: &AgentRuntime) -> Vec<String> {
    agent
        .resolve_tools_for_run()
        .await
        .expect("tools should resolve")
        .iter()
        .map(|tool| tool.name().to_string())
        .collect()
}
