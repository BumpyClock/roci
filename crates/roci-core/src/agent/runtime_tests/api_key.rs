use super::support::*;
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[tokio::test]
async fn prompt_get_api_key_error_restores_idle_state() {
    let get_key: GetApiKeyFn = Arc::new(|_model| {
        Box::pin(async {
            Err(RociError::Authentication(
                "Token refresh failed".to_string(),
            ))
        })
    });
    let agent = AgentRuntime::new(
        test_registry(),
        test_config(),
        AgentConfig {
            get_api_key: Some(get_key),
            ..test_agent_config()
        },
    );

    let err = agent.prompt("hello").await.unwrap_err();
    assert!(matches!(
        err,
        RociError::Authentication(msg) if msg == "Token refresh failed"
    ));
    assert_eq!(agent.state().await, AgentState::Idle);

    // Must not block after a failed prompt.
    agent.wait_for_idle().await;

    let snap = agent.snapshot().await;
    assert_eq!(snap.state, AgentState::Idle);
    assert!(!snap.is_streaming);
    assert_eq!(
        snap.last_error,
        Some("Authentication error: Token refresh failed".into())
    );
}

#[tokio::test]
async fn prompt_resolves_keys_in_request_config_callback_order() {
    for (config_key, override_key, with_callback, expected) in [
        (Some("sk-config"), None, false, "sk-config"),
        (Some("sk-config"), None, true, "sk-config"),
        (Some("sk-config"), Some("sk-override"), true, "sk-override"),
        (None, Some("sk-override"), true, "sk-override"),
        (None, None, true, "sk-callback"),
    ] {
        let (registry, requests) = registry_with_recorded_requests();
        let roci_config = RociConfig::new().with_token_store(None);
        if let Some(key) = config_key {
            roci_config.set_api_key("stub", key.into());
        }
        let callback_calls = Arc::new(AtomicUsize::new(0));
        let calls = callback_calls.clone();
        let get_key: GetApiKeyFn = Arc::new(move |model| {
            assert_eq!(model.provider_name(), "stub");
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok("sk-callback".into()) })
        });
        let agent = AgentRuntime::new(
            registry,
            roci_config,
            AgentConfig {
                candidates: vec!["stub:api-key".parse().unwrap()],
                api_key_override: override_key.map(String::from),
                get_api_key: with_callback.then_some(get_key),
                ..test_agent_config()
            },
        );

        let result = agent.prompt("hello").await.unwrap();
        assert_eq!(result.status, RunStatus::Completed);
        let requests = requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].1.as_deref(), Some(expected));
        assert_eq!(
            callback_calls.load(Ordering::SeqCst),
            usize::from(expected == "sk-callback")
        );
    }
}

#[tokio::test]
async fn prompt_resolves_rotated_callback_key_for_each_run() {
    let (registry, requests) = registry_with_recorded_requests();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let get_key: GetApiKeyFn = Arc::new(move |_model| {
        let index = counter.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { Ok(format!("sk-key-{index}")) })
    });
    let agent = AgentRuntime::new(
        registry,
        RociConfig::new().with_token_store(None),
        AgentConfig {
            candidates: vec!["stub:api-key".parse().unwrap()],
            get_api_key: Some(get_key),
            ..test_agent_config()
        },
    );

    for _ in 0..3 {
        assert_eq!(
            agent.prompt("hello").await.unwrap().status,
            RunStatus::Completed
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let requests = requests.lock().expect("requests lock");
    assert_eq!(
        requests
            .iter()
            .map(|(_, key)| key.as_deref())
            .collect::<Vec<_>>(),
        [Some("sk-key-0"), Some("sk-key-1"), Some("sk-key-2")]
    );
}
