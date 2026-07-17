//! Tests for parent-host sub-agent management APIs on `AgentRuntime`.

use uuid::Uuid;

use super::support::{test_agent_config, test_config, test_registry};
use crate::agent::runtime::{AgentRuntime, AgentSubagentConfig};
use crate::agent::subagents::SubagentProfileRegistry;
use crate::error::RociError;

fn assert_subagents_disabled(error: RociError) {
    assert!(matches!(
        error,
        RociError::UnsupportedOperation(message)
            if message == "subagent management is not enabled for this AgentRuntime"
    ));
}

#[tokio::test]
async fn subagent_host_apis_return_unsupported_when_disabled() {
    let runtime = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    assert_subagents_disabled(runtime.list_subagents().await.unwrap_err());
    assert_subagents_disabled(runtime.list_subagent_profiles().unwrap_err());
    assert_subagents_disabled(runtime.current_subagent_profile().await.unwrap_err());
    assert_subagents_disabled(
        runtime
            .select_subagent_profile("builtin:developer")
            .await
            .unwrap_err(),
    );
    assert_subagents_disabled(runtime.deselect_subagent_profile().await.unwrap_err());
    assert_subagents_disabled(runtime.cancel_subagent(Uuid::nil()).await.unwrap_err());
    assert_subagents_disabled(
        runtime
            .send_subagent_message(Uuid::nil(), "status?")
            .await
            .unwrap_err(),
    );
}

#[tokio::test]
async fn subagent_host_apis_delegate_to_enabled_controller() {
    let mut config = test_agent_config();
    config.subagents = Some(Default::default());
    let runtime = AgentRuntime::new(test_registry(), test_config(), config);

    assert!(runtime.list_subagents().await.unwrap().is_empty());
    assert!(runtime.current_subagent_profile().await.unwrap().is_none());
    let profiles = runtime.list_subagent_profiles().unwrap();
    assert!(profiles.is_empty());
    assert!(matches!(
        runtime.cancel_subagent(Uuid::nil()).await,
        Err(RociError::Configuration(message)) if message.contains("not found")
    ));
    assert!(matches!(
        runtime.send_subagent_message(Uuid::nil(), "status?").await,
        Err(RociError::Configuration(message)) if message.contains("not found")
    ));
}

#[tokio::test]
async fn subagent_profile_host_apis_manage_selected_override() {
    let mut config = test_agent_config();
    config.subagents = Some(AgentSubagentConfig {
        profiles: SubagentProfileRegistry::with_builtins(),
        main_profile: Some("builtin:developer".into()),
        ..AgentSubagentConfig::default()
    });
    let runtime = AgentRuntime::new(test_registry(), test_config(), config);

    assert_eq!(runtime.list_subagent_profiles().unwrap().len(), 3);
    assert_eq!(
        runtime
            .current_subagent_profile()
            .await
            .unwrap()
            .unwrap()
            .name,
        "builtin:developer"
    );

    let selected = runtime
        .select_subagent_profile("builtin:planner")
        .await
        .unwrap();
    assert_eq!(selected.name, "builtin:planner");

    runtime.deselect_subagent_profile().await.unwrap();
    assert!(runtime.current_subagent_profile().await.unwrap().is_none());
}
