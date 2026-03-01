use super::support::*;
use super::*;
use std::sync::Arc;

#[tokio::test]
async fn get_api_key_callback_returns_resolved_key() {
    let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = call_count.clone();

    let get_key: GetApiKeyFn = Arc::new(move || {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { Ok("sk-live-rotated-key".to_string()) })
    });

    let key = get_key().await.unwrap();
    assert_eq!(key, "sk-live-rotated-key");
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);

    let key2 = get_key().await.unwrap();
    assert_eq!(key2, "sk-live-rotated-key");
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn static_key_works_without_callback() {
    let config = AgentConfig {
        get_api_key: None,
        ..test_agent_config()
    };
    let agent = AgentRuntime::new(test_registry(), test_config(), config);

    assert!(agent.config.get_api_key.is_none());
    assert_eq!(agent.state().await, AgentState::Idle);
}

#[tokio::test]
async fn get_api_key_error_propagates() {
    let get_key: GetApiKeyFn = Arc::new(|| {
        Box::pin(async {
            Err(RociError::Authentication(
                "Token refresh failed".to_string(),
            ))
        })
    });

    let result = get_key().await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        RociError::Authentication(msg) if msg == "Token refresh failed"
    ));
}

#[tokio::test]
async fn prompt_get_api_key_error_restores_idle_state() {
    let get_key: GetApiKeyFn = Arc::new(|| {
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
async fn agent_runtime_uses_config_api_key_by_default() {
    let roci_config = RociConfig::new().with_token_store(None);
    roci_config.set_api_key("openai", "sk-from-config".to_string());

    let agent_config = AgentConfig {
        get_api_key: None,
        ..test_agent_config()
    };
    let agent = AgentRuntime::new(test_registry(), roci_config, agent_config);

    assert!(agent.config.get_api_key.is_none());
    assert_eq!(agent.state().await, AgentState::Idle);
}

#[tokio::test]
async fn get_api_key_callback_is_skipped_when_config_key_exists() {
    let roci_config = RociConfig::new().with_token_store(None);
    roci_config.set_api_key("openai", "sk-from-config".to_string());
    let callback_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let callback_calls_for_hook = callback_calls.clone();
    let get_key: GetApiKeyFn = Arc::new(move || {
        let callback_calls_for_hook = callback_calls_for_hook.clone();
        Box::pin(async move {
            callback_calls_for_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("sk-from-callback".to_string())
        })
    });

    let agent = AgentRuntime::new(
        test_registry(),
        roci_config,
        AgentConfig {
            get_api_key: Some(get_key),
            ..test_agent_config()
        },
    );

    let _ = agent.prompt("hello").await;
    assert_eq!(
        callback_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "config key should take precedence over callback"
    );
}

#[tokio::test]
async fn get_api_key_callback_is_skipped_when_request_override_exists() {
    let callback_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let callback_calls_for_hook = callback_calls.clone();
    let get_key: GetApiKeyFn = Arc::new(move || {
        let callback_calls_for_hook = callback_calls_for_hook.clone();
        Box::pin(async move {
            callback_calls_for_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("sk-from-callback".to_string())
        })
    });

    let agent = AgentRuntime::new(
        test_registry(),
        test_config(),
        AgentConfig {
            api_key_override: Some("sk-request-override".to_string()),
            get_api_key: Some(get_key),
            ..test_agent_config()
        },
    );

    let _ = agent.prompt("hello").await;
    assert_eq!(
        callback_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "request override should take precedence over callback"
    );
}

#[tokio::test]
async fn get_api_key_callback_can_rotate_keys() {
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter_clone = counter.clone();

    let get_key: GetApiKeyFn = Arc::new(move || {
        let n = counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move { Ok(format!("sk-key-{}", n)) })
    });

    assert_eq!(get_key().await.unwrap(), "sk-key-0");
    assert_eq!(get_key().await.unwrap(), "sk-key-1");
    assert_eq!(get_key().await.unwrap(), "sk-key-2");
}
