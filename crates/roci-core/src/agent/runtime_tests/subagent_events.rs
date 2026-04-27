//! Tests for sub-agent event forwarding and user input coordination.

use tokio::sync::broadcast;
use uuid::Uuid;

use crate::agent::subagents::events::build_child_event_sink;
use crate::agent::subagents::types::{SubagentEvent, SubagentId};
use crate::agent_loop::AgentEvent;

// ---------------------------------------------------------------------------
// build_child_event_sink
// ---------------------------------------------------------------------------

#[test]
fn event_sink_wraps_agent_event_with_label() {
    let (tx, mut rx) = broadcast::channel(16);
    let id: SubagentId = Uuid::new_v4();
    let sink = build_child_event_sink(id, Some("worker-1".into()), tx);

    let event = AgentEvent::AgentStart {
        run_id: Uuid::new_v4(),
    };
    sink(event);

    let received = rx.try_recv().unwrap();
    match received {
        SubagentEvent::AgentEvent {
            subagent_id,
            label,
            event: boxed_event,
        } => {
            assert_eq!(subagent_id, id);
            assert_eq!(label.as_deref(), Some("worker-1"));
            assert!(matches!(*boxed_event, AgentEvent::AgentStart { .. }));
        }
        _ => panic!("expected SubagentEvent::AgentEvent"),
    }
}

#[test]
fn event_sink_wraps_agent_event_without_label() {
    let (tx, mut rx) = broadcast::channel(16);
    let id: SubagentId = Uuid::new_v4();
    let sink = build_child_event_sink(id, None, tx);

    sink(AgentEvent::AgentStart {
        run_id: Uuid::new_v4(),
    });

    let received = rx.try_recv().unwrap();
    match received {
        SubagentEvent::AgentEvent {
            subagent_id, label, ..
        } => {
            assert_eq!(subagent_id, id);
            assert!(label.is_none());
        }
        _ => panic!("expected SubagentEvent::AgentEvent"),
    }
}

#[test]
fn event_sink_forwards_multiple_events() {
    let (tx, mut rx) = broadcast::channel(16);
    let id: SubagentId = Uuid::new_v4();
    let sink = build_child_event_sink(id, Some("multi".into()), tx);

    sink(AgentEvent::AgentStart {
        run_id: Uuid::new_v4(),
    });
    sink(AgentEvent::AgentEnd {
        run_id: Uuid::new_v4(),
        messages: Vec::new(),
    });

    let first = rx.try_recv().unwrap();
    let second = rx.try_recv().unwrap();

    match first {
        SubagentEvent::AgentEvent { event, .. } => {
            assert!(matches!(*event, AgentEvent::AgentStart { .. }));
        }
        _ => panic!("expected AgentEvent for first"),
    }
    match second {
        SubagentEvent::AgentEvent { event, .. } => {
            assert!(matches!(*event, AgentEvent::AgentEnd { .. }));
        }
        _ => panic!("expected AgentEvent for second"),
    }
}

#[test]
fn event_sink_does_not_panic_when_no_receivers() {
    let (tx, _) = broadcast::channel::<SubagentEvent>(16);
    let id: SubagentId = Uuid::new_v4();
    let sink = build_child_event_sink(id, None, tx);

    // Drop the receiver -- sink should silently discard
    sink(AgentEvent::AgentStart {
        run_id: Uuid::new_v4(),
    });
}

// ---------------------------------------------------------------------------
// submit_user_input with unknown request
// ---------------------------------------------------------------------------

#[cfg(feature = "agent")]
#[tokio::test]
async fn submit_user_input_unknown_request_returns_error() {
    use crate::agent::subagents::profiles::SubagentProfileRegistry;
    use crate::agent::subagents::supervisor::SubagentSupervisor;
    use crate::agent::subagents::types::SubagentSupervisorConfig;
    use crate::tools::UserInputResponse;
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::agent::runtime::AgentConfig;
    use crate::config::RociConfig;
    use crate::models::LanguageModel;
    use crate::provider::ProviderRegistry;

    let model = LanguageModel::Known {
        provider_key: "test".into(),
        model_id: "test-model".into(),
    };
    let config = AgentConfig {
        model,
        system_prompt: None,
        tools: Vec::new(),
        dynamic_tool_providers: Vec::new(),
        settings: crate::types::GenerationSettings::default(),
        transform_context: None,
        convert_to_llm: None,
        before_agent_start: None,
        event_sink: None,
        session_id: None,
        steering_mode: crate::agent::runtime::QueueDrainMode::All,
        follow_up_mode: crate::agent::runtime::QueueDrainMode::All,
        transport: None,
        max_retry_delay_ms: None,
        retry_backoff: crate::agent_loop::runner::RetryBackoffPolicy::default(),
        api_key_override: None,
        provider_headers: reqwest::header::HeaderMap::new(),
        provider_metadata: HashMap::new(),
        provider_payload_callback: None,
        get_api_key: None,
        compaction: crate::resource::CompactionSettings::default(),
        session_before_compact: None,
        session_before_tree: None,
        pre_tool_use: None,
        post_tool_use: None,
        user_input_timeout_ms: None,
        user_input_coordinator: None,
        context_budget: None,
        chat: Default::default(),
    };

    let supervisor = SubagentSupervisor::new(
        Arc::new(ProviderRegistry::new()),
        RociConfig::default(),
        config,
        SubagentSupervisorConfig::default(),
        SubagentProfileRegistry::with_builtins(),
    );

    let response = UserInputResponse {
        request_id: Uuid::nil(),
        answers: vec![],
        canceled: false,
    };
    let result = supervisor.submit_user_input(response).await;
    assert!(result.is_err());
}
