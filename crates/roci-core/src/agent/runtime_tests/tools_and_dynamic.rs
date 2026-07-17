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
#[cfg(feature = "agent")]
use tokio::sync::Notify;

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
async fn subagent_runtime_profile_selection_updates_tool_visibility_for_next_run() {
    let mut profiles = SubagentProfileRegistry::new();
    profiles
        .register(SubagentProfile {
            name: "test:alpha".into(),
            default_agent_excluded_tools: vec!["danger_tool".into()],
            default: true,
            ..SubagentProfile::default()
        })
        .unwrap();
    profiles
        .register(SubagentProfile {
            name: "test:beta".into(),
            default_agent_excluded_tools: vec!["safe_tool".into()],
            ..SubagentProfile::default()
        })
        .unwrap();
    let mut config = test_agent_config();
    config.tools = vec![dummy_tool("safe_tool"), dummy_tool("danger_tool")];
    config.tool_visibility_policy =
        ToolVisibilityPolicy::allow_only(["safe_tool", "danger_tool", "delegate_subagent"]);
    config.subagents = Some(AgentSubagentConfig {
        profiles,
        supervisor: SubagentSupervisorConfig::default(),
        enabled: true,
        main_profile: None,
    });
    let agent = AgentRuntime::new(test_registry(), test_config(), config);

    let default_names = resolved_tool_names(&agent).await;
    assert!(default_names.contains(&"safe_tool".to_string()));
    assert!(!default_names.contains(&"danger_tool".to_string()));

    agent.select_subagent_profile("test:beta").await.unwrap();
    let selected_names = resolved_tool_names(&agent).await;
    assert!(!selected_names.contains(&"safe_tool".to_string()));
    assert!(selected_names.contains(&"danger_tool".to_string()));

    agent.deselect_subagent_profile().await.unwrap();
    let deselected_names = resolved_tool_names(&agent).await;
    assert!(deselected_names.contains(&"safe_tool".to_string()));
    assert!(!deselected_names.contains(&"danger_tool".to_string()));
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

#[cfg(feature = "agent")]
#[tokio::test]
async fn subagent_runtime_bridge_persists_more_than_broadcast_capacity_without_gaps() {
    const STREAM_CHUNKS: usize = 300;

    let mut config = test_agent_config();
    config.subagents = Some(test_subagent_config(true, None));
    config.api_key_override = Some("test-key".into());
    config.chat.replay_capacity = 1_024;
    let agent = AgentRuntime::new(
        registry_with_streaming_chunks_provider("test", STREAM_CHUNKS),
        test_config(),
        config,
    );
    agent.ensure_runtime_event_publisher().await;
    let thread_id = agent.read_snapshot().await.threads[0].thread_id;
    let mut live = agent.subscribe(None).await;
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
                "task": "stream enough events to overflow the old broadcast bridge",
                "run_in_background": false
            })),
            &ToolExecutionContext::default(),
        )
        .await
        .expect("delegate tool should execute");
    let subagent_id: crate::agent::subagents::SubagentId = result["subagent_id"]
        .as_str()
        .expect("result should contain subagent id")
        .parse()
        .expect("subagent id should parse");

    let mut live_subagent_events = Vec::new();
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), live.recv())
            .await
            .expect("semantic subagent event should arrive")
            .expect("runtime event should decode");
        let is_terminal = matches!(
            &event.payload,
            AgentRuntimeEventPayload::SubagentCompleted { subagent, .. }
                if subagent.subagent_id == subagent_id
        );
        if subagent_snapshot(&event.payload)
            .is_some_and(|subagent| subagent.subagent_id == subagent_id)
        {
            live_subagent_events.push(event);
        }
        if is_terminal {
            break;
        }
    }

    let replayed = agent
        .subscribe(Some(RuntimeCursor::new(thread_id, 0)))
        .await
        .replay()
        .expect("subagent events should replay");
    let replayed_subagent_events = replayed
        .into_iter()
        .filter(|event| {
            subagent_snapshot(&event.payload)
                .is_some_and(|subagent| subagent.subagent_id == subagent_id)
        })
        .collect::<Vec<_>>();

    assert_eq!(replayed_subagent_events, live_subagent_events);
    assert_eq!(replayed_subagent_events.len(), STREAM_CHUNKS + 6);
    assert_eq!(
        replayed_subagent_events
            .iter()
            .filter(|event| matches!(
                event.payload,
                AgentRuntimeEventPayload::SubagentStarted { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        replayed_subagent_events
            .iter()
            .filter(|event| matches!(
                event.payload,
                AgentRuntimeEventPayload::SubagentProgress { .. }
            ))
            .count(),
        STREAM_CHUNKS + 1
    );
    let sequences = replayed_subagent_events
        .iter()
        .map(|event| {
            subagent_snapshot(&event.payload)
                .expect("filtered event should contain subagent snapshot")
                .sequence
        })
        .collect::<Vec<_>>();
    assert_eq!(
        sequences,
        (1..=u64::try_from(sequences.len()).unwrap()).collect::<Vec<_>>()
    );
}

#[cfg(feature = "agent")]
#[tokio::test]
async fn subagent_event_never_attaches_to_unrelated_active_parent_turn() {
    let gate = Arc::new(Notify::new());
    let mut config = test_agent_config();
    config.subagents = Some(test_subagent_config(true, None));
    config.api_key_override = Some("test-key".into());
    let agent = AgentRuntime::new(
        registry_with_gated_streaming_provider("test", gate.clone()),
        test_config(),
        config,
    );
    agent.ensure_runtime_event_publisher().await;
    let mut events = agent.subscribe(None).await;
    let parent_turn_id = {
        let mut projector = agent
            .chat_projector
            .lock()
            .expect("chat projector lock should not be poisoned");
        let turn_id = projector
            .queue_turn(vec![ModelMessage::user("delegate")])
            .turn_id;
        projector
            .start_turn(turn_id)
            .expect("parent turn should start");
        projector
            .start_tool(
                turn_id,
                "parent-call-1",
                "delegate_subagent",
                serde_json::Value::Null,
            )
            .expect("parent delegate tool should start");
        turn_id
    };
    let delegate_tool = agent
        .resolve_tools_for_run()
        .await
        .expect("tools should resolve")
        .into_iter()
        .find(|tool| tool.name() == "delegate_subagent")
        .expect("delegate tool should be injected");
    let projector = agent.chat_projector.clone();
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
    let (rotate_tx, rotate_rx) = std::sync::mpsc::channel();
    let rotate_parent = tokio::task::spawn_blocking(move || {
        let mut projector = projector
            .lock()
            .expect("chat projector lock should not be poisoned");
        locked_tx
            .send(())
            .expect("test should wait for projector lock");
        rotate_rx
            .recv()
            .expect("test should request parent rotation");
        projector
            .complete_tool(
                parent_turn_id,
                "parent-call-1",
                crate::types::AgentToolResult {
                    tool_call_id: "parent-call-1".to_string(),
                    result: serde_json::Value::Null,
                    is_error: false,
                },
            )
            .expect("parent delegate tool should complete");
        projector
            .complete_turn(parent_turn_id)
            .expect("original parent turn should complete");
        let unrelated_turn_id = projector
            .queue_turn(vec![ModelMessage::user("unrelated")])
            .turn_id;
        projector
            .start_turn(unrelated_turn_id)
            .expect("unrelated parent turn should start");
        unrelated_turn_id
    });
    locked_rx.await.expect("projector lock holder should start");

    delegate_tool
        .execute(
            &ToolArguments::new(serde_json::json!({
                "task": "finish after release",
                "run_in_background": true
            })),
            &ToolExecutionContext {
                tool_call_id: Some("parent-call-1".to_string()),
                tool_name: Some("delegate_subagent".to_string()),
                ..ToolExecutionContext::default()
            },
        )
        .await
        .expect("delegate tool should execute");
    rotate_tx
        .send(())
        .expect("parent rotation task should still run");
    let unrelated_turn_id = rotate_parent
        .await
        .expect("parent rotation task should complete");

    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("subagent started event should arrive")
            .expect("runtime event should decode");
        if let AgentRuntimeEventPayload::SubagentStarted { subagent } = event.payload {
            assert_ne!(event.turn_id, Some(unrelated_turn_id));
            assert_eq!(event.turn_id, None);
            assert_eq!(subagent.parent_turn_id, Some(parent_turn_id));
            break;
        }
    }

    gate.notify_one();

    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("subagent completed event should arrive")
            .expect("runtime event should decode");
        if let AgentRuntimeEventPayload::SubagentCompleted { subagent, .. } = event.payload {
            assert_ne!(event.turn_id, Some(unrelated_turn_id));
            assert_eq!(event.turn_id, None);
            assert_eq!(subagent.parent_turn_id, Some(parent_turn_id));
            break;
        }
    }
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

#[cfg(feature = "agent")]
fn subagent_snapshot(
    payload: &AgentRuntimeEventPayload,
) -> Option<&crate::agent::runtime::SubagentRuntimeSnapshot> {
    match payload {
        AgentRuntimeEventPayload::SubagentStarted { subagent }
        | AgentRuntimeEventPayload::SubagentProgress { subagent, .. }
        | AgentRuntimeEventPayload::SubagentToolCallStarted { subagent, .. }
        | AgentRuntimeEventPayload::SubagentToolCallCompleted { subagent, .. }
        | AgentRuntimeEventPayload::SubagentMessage { subagent, .. }
        | AgentRuntimeEventPayload::SubagentNeedsInput { subagent, .. }
        | AgentRuntimeEventPayload::SubagentInputResolved { subagent, .. }
        | AgentRuntimeEventPayload::SubagentInputCanceled { subagent, .. }
        | AgentRuntimeEventPayload::SubagentCompleted { subagent, .. }
        | AgentRuntimeEventPayload::SubagentFailed { subagent, .. }
        | AgentRuntimeEventPayload::SubagentCancelled { subagent } => Some(subagent),
        _ => None,
    }
}
