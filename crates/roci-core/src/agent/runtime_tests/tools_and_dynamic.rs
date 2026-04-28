use super::support::*;
use super::*;
use crate::tools::dynamic::{DynamicTool, DynamicToolProvider};
use crate::tools::{AgentToolParameters, ToolApproval, ToolApprovalKind};
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
        Arc::new(MockDynamicToolProvider::new(vec![DynamicTool {
            name: "dynamic".into(),
            description: "dynamic tool".into(),
            parameters: AgentToolParameters::empty(),
            approval: ToolApproval::requires_approval(ToolApprovalKind::Other),
        }]));

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
