//! Tests for SubagentSupervisor lifecycle and concurrency.
//!
//! Drop-behavior tests and internal-access tests live in
//! `supervisor.rs`'s own `mod tests` block. These tests exercise the
//! public API only.

use std::collections::HashMap;
use std::sync::Arc;

use uuid::Uuid;

use crate::agent::runtime::AgentConfig;
use crate::agent::subagents::profiles::SubagentProfileRegistry;
use crate::agent::subagents::supervisor::SubagentSupervisor;
use crate::agent::subagents::types::{
    SubagentInput, SubagentSpec, SubagentStatus, SubagentSupervisorConfig,
};
use crate::config::RociConfig;
use crate::models::LanguageModel;
use crate::provider::ProviderRegistry;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_test_model() -> LanguageModel {
    LanguageModel::Known {
        provider_key: "test".into(),
        model_id: "test-model".into(),
    }
}

fn make_base_config() -> AgentConfig {
    use crate::agent::runtime::QueueDrainMode;
    use crate::agent_loop::runner::RetryBackoffPolicy;
    use crate::resource::CompactionSettings;
    use crate::types::GenerationSettings;

    AgentConfig {
        model: make_test_model(),
        system_prompt: None,
        tools: Vec::new(),
        dynamic_tool_providers: Vec::new(),
        settings: GenerationSettings::default(),
        transform_context: None,
        convert_to_llm: None,
        before_agent_start: None,
        event_sink: None,
        session_id: None,
        steering_mode: QueueDrainMode::All,
        follow_up_mode: QueueDrainMode::All,
        transport: None,
        max_retry_delay_ms: None,
        retry_backoff: RetryBackoffPolicy::default(),
        api_key_override: None,
        provider_headers: reqwest::header::HeaderMap::new(),
        provider_metadata: HashMap::new(),
        provider_payload_callback: None,
        get_api_key: None,
        compaction: CompactionSettings::default(),
        session_before_compact: None,
        session_before_tree: None,
        pre_tool_use: None,
        post_tool_use: None,
        user_input_timeout_ms: None,
        #[cfg(feature = "agent")]
        user_input_coordinator: None,
    }
}

fn make_supervisor() -> SubagentSupervisor {
    let registry = Arc::new(ProviderRegistry::new());
    let roci_config = RociConfig::default();
    let base_config = make_base_config();
    let sup_config = SubagentSupervisorConfig::default();
    let profile_registry = SubagentProfileRegistry::with_builtins();
    SubagentSupervisor::new(
        registry,
        roci_config,
        base_config,
        sup_config,
        profile_registry,
    )
}

fn make_supervisor_with_config(sup_config: SubagentSupervisorConfig) -> SubagentSupervisor {
    let registry = Arc::new(ProviderRegistry::new());
    let roci_config = RociConfig::default();
    let base_config = make_base_config();
    let profile_registry = SubagentProfileRegistry::with_builtins();
    SubagentSupervisor::new(
        registry,
        roci_config,
        base_config,
        sup_config,
        profile_registry,
    )
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

#[test]
fn supervisor_construction_with_default_config() {
    let supervisor = make_supervisor();
    // Should not panic; default max_concurrent is 4
    let _rx = supervisor.subscribe();
}

#[test]
fn supervisor_construction_with_custom_config() {
    let config = SubagentSupervisorConfig {
        max_concurrent: 2,
        max_active_children: Some(10),
        default_input_timeout_ms: Some(60_000),
        abort_on_drop: false,
    };
    let _supervisor = make_supervisor_with_config(config);
}

// ---------------------------------------------------------------------------
// list_active
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_active_empty_on_fresh_supervisor() {
    let supervisor = make_supervisor();
    let active = supervisor.list_active().await;
    assert!(active.is_empty());
}

// ---------------------------------------------------------------------------
// subscribe
// ---------------------------------------------------------------------------

#[test]
fn subscribe_returns_a_receiver() {
    let supervisor = make_supervisor();
    let _rx = supervisor.subscribe();
    // Subscribing again should also work
    let _rx2 = supervisor.subscribe();
}

#[test]
fn subscribe_receiver_is_empty_initially() {
    let supervisor = make_supervisor();
    let mut rx = supervisor.subscribe();
    // No events yet -- try_recv should return TryRecvError
    assert!(rx.try_recv().is_err());
}

// ---------------------------------------------------------------------------
// wait_any / wait_all with no children
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wait_any_returns_none_when_no_active_children() {
    let supervisor = make_supervisor();
    assert!(supervisor.wait_any().await.is_none());
}

#[tokio::test]
async fn wait_all_returns_empty_when_no_active_children() {
    let supervisor = make_supervisor();
    let results = supervisor.wait_all().await;
    assert!(results.is_empty());
}

// ---------------------------------------------------------------------------
// max_active_children cap (via spawn rejection)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_rejected_when_max_active_children_zero() {
    // max_active_children = 0 means no children can ever be spawned
    let config = SubagentSupervisorConfig {
        max_active_children: Some(0),
        ..Default::default()
    };
    let supervisor = make_supervisor_with_config(config);

    let spec = SubagentSpec {
        profile: "builtin:developer".into(),
        label: Some("test".into()),
        input: SubagentInput::Prompt {
            task: "hello".into(),
        },
        overrides: Default::default(),
    };

    let result = supervisor.spawn(spec).await;
    let err = result.err().expect("expected spawn to fail");
    assert!(err.to_string().contains("max active children"));
}

// ---------------------------------------------------------------------------
// spawn with unknown profile
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_with_unknown_profile_returns_error() {
    let supervisor = make_supervisor();
    let spec = SubagentSpec {
        profile: "nonexistent-profile".into(),
        label: None,
        input: SubagentInput::Prompt {
            task: "test".into(),
        },
        overrides: Default::default(),
    };
    let result = supervisor.spawn(spec).await;
    let err = result.err().expect("expected spawn to fail");
    assert!(err.to_string().contains("not found"));
}

// ---------------------------------------------------------------------------
// shutdown on empty supervisor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shutdown_on_empty_supervisor_is_fine() {
    let supervisor = make_supervisor();
    supervisor.shutdown().await;
    // Should not hang or panic
    let active = supervisor.list_active().await;
    assert!(active.is_empty());
}

// ---------------------------------------------------------------------------
// abort / wait on unknown child
// ---------------------------------------------------------------------------

#[tokio::test]
async fn abort_unknown_child_returns_error() {
    let supervisor = make_supervisor();
    let result = supervisor.abort(Uuid::new_v4()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn wait_unknown_child_returns_error() {
    let supervisor = make_supervisor();
    let result = supervisor.wait(Uuid::new_v4()).await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// submit_user_input with unknown request
// ---------------------------------------------------------------------------

#[cfg(feature = "agent")]
#[tokio::test]
async fn submit_user_input_unknown_request_returns_error() {
    use crate::tools::UserInputResponse;

    let supervisor = make_supervisor();
    let response = UserInputResponse {
        request_id: Uuid::nil(),
        answers: vec![],
        canceled: false,
    };
    let result = supervisor.submit_user_input(response).await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Spawn tests that need a real provider (ignored)
// ---------------------------------------------------------------------------

/// Full spawn -> wait lifecycle test. Requires API keys and a configured
/// provider with valid credentials.
#[ignore = "requires API keys and a configured provider"]
#[tokio::test]
async fn spawn_and_wait_full_lifecycle() {
    let supervisor = make_supervisor();
    let spec = SubagentSpec {
        profile: "builtin:developer".into(),
        label: Some("live-test".into()),
        input: SubagentInput::Prompt {
            task: "say hello".into(),
        },
        overrides: Default::default(),
    };
    let handle = supervisor.spawn(spec).await.unwrap();
    let result = handle.wait().await;
    assert!(matches!(
        result.status,
        SubagentStatus::Completed | SubagentStatus::Failed
    ));
}
