use super::*;

#[tokio::test]
async fn list_tools_reconnects_when_session_disconnects() {
    let transport = MockBootstrapTransport::new(vec![
        Ok(scripted_running_service(
            MockSessionBehavior::DisconnectOnListTools,
        )),
        Ok(scripted_running_service(MockSessionBehavior::ListTools {
            tool_name: "weather".into(),
        })),
    ]);
    let attempted = transport.attempted_protocols();
    let mut client = MCPClient::new(Box::new(transport));

    client
        .initialize()
        .await
        .expect("initialize should succeed");
    let tools = client
        .list_tools()
        .await
        .expect("list_tools should reconnect and retry");

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "weather");

    let attempted = attempted.lock().expect("protocol mutex should lock");
    assert_eq!(attempted.len(), 2);
}

#[tokio::test]
async fn list_resources_reconnects_and_refreshes_session_derived_view() {
    let transport = MockBootstrapTransport::new(vec![
        Ok(scripted_running_service(
            MockSessionBehavior::DisconnectOnListResources,
        )),
        Ok(scripted_running_service(
            MockSessionBehavior::ListResources {
                resource_name: "fresh".into(),
            },
        )),
    ]);
    let attempted = transport.attempted_protocols();
    let mut client = MCPClient::new(Box::new(transport));

    client
        .initialize()
        .await
        .expect("initialize should succeed");
    let resources = client
        .list_resources()
        .await
        .expect("list_resources should reconnect and retry");

    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].name, "fresh");
    assert_eq!(
        client.last_reconnect_outcome(),
        Some(MCPRemoteReconnectOutcome::Recovered)
    );

    let attempted = attempted.lock().expect("protocol mutex should lock");
    assert_eq!(attempted.len(), 2);
}

#[tokio::test]
async fn session_expired_reconnects_with_fast_refresh_attempt() {
    let transport = MockBootstrapTransport::new(vec![
        Ok(scripted_running_service(
            MockSessionBehavior::SessionExpiredOnListTools,
        )),
        Ok(scripted_running_service(MockSessionBehavior::ListTools {
            tool_name: "fresh".into(),
        })),
    ])
    .with_reconnect_policy(MCPRemoteReconnectPolicy {
        initial_backoff_ms: 10_000,
        jitter_ratio: 0.0,
        ..MCPRemoteReconnectPolicy::default()
    });
    let attempted = transport.attempted_protocols();
    let mut client = MCPClient::new(Box::new(transport));

    client
        .initialize()
        .await
        .expect("initialize should succeed");
    let started = Instant::now();
    let tools = client
        .list_tools()
        .await
        .expect("session expired should reconnect immediately");

    assert_eq!(tools[0].name, "fresh");
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(
        client.last_reconnect_outcome(),
        Some(MCPRemoteReconnectOutcome::Recovered)
    );
    let attempted = attempted.lock().expect("protocol mutex should lock");
    assert_eq!(attempted.len(), 2);
}

#[tokio::test]
async fn call_tool_reconnects_when_session_disconnects() {
    let transport = MockBootstrapTransport::new(vec![
        Ok(scripted_running_service(
            MockSessionBehavior::DisconnectOnCallTool,
        )),
        Ok(scripted_running_service(MockSessionBehavior::CallTool)),
    ]);
    let attempted = transport.attempted_protocols();
    let mut client = MCPClient::new(Box::new(transport));

    client
        .initialize()
        .await
        .expect("initialize should succeed");
    let result = client
        .call_tool("echo", json!({"message": "hello"}))
        .await
        .expect("call_tool should reconnect and retry");

    assert_eq!(result.structured_content, Some(json!({"ok": true})));

    let attempted = attempted.lock().expect("protocol mutex should lock");
    assert_eq!(attempted.len(), 2);
}

#[tokio::test]
async fn call_tool_timeout_reconnects_without_replaying_operation() {
    let transport = MockBootstrapTransport::new(vec![
        Ok(scripted_running_service(MockSessionBehavior::CallTool)),
        Ok(scripted_running_service(MockSessionBehavior::CallTool)),
    ]);
    let attempted = transport.attempted_protocols();
    let mut client = MCPClient::new(Box::new(transport));

    client
        .initialize()
        .await
        .expect("initialize should succeed");

    let calls = Arc::new(Mutex::new(0usize));
    let calls_for_operation = Arc::clone(&calls);
    let error = client
        .with_reconnect::<(), _>(
            "call_tool",
            move |_client| {
                let calls = Arc::clone(&calls_for_operation);
                Box::pin(async move {
                    let mut count = calls.lock().expect("call count mutex should lock");
                    *count += 1;
                    Err(ServiceError::Timeout {
                        timeout: Duration::from_millis(25),
                    })
                })
            },
            ReconnectReplayPolicy::DoNotReplayTimeouts,
        )
        .await
        .expect_err("timed-out tool call should not be replayed");

    assert!(matches!(error, RociError::Timeout(25)));
    assert_eq!(*calls.lock().expect("call count mutex should lock"), 1);
    assert_eq!(
        client.last_reconnect_outcome(),
        Some(MCPRemoteReconnectOutcome::Recovered)
    );
    let attempted = attempted.lock().expect("protocol mutex should lock");
    assert_eq!(attempted.len(), 2);
}

#[tokio::test]
async fn list_tools_reconnects_after_idle_timeout() {
    let policy = MCPRemoteReconnectPolicy {
        idle_timeout_ms: Some(1),
        jitter_ratio: 0.0,
        ..MCPRemoteReconnectPolicy::default()
    };
    let transport = MockBootstrapTransport::new(vec![
        Ok(scripted_running_service(MockSessionBehavior::ListTools {
            tool_name: "stale".into(),
        })),
        Ok(scripted_running_service(MockSessionBehavior::ListTools {
            tool_name: "fresh".into(),
        })),
    ])
    .with_reconnect_policy(policy);
    let attempted = transport.attempted_protocols();
    let mut client = MCPClient::new(Box::new(transport));

    client
        .initialize()
        .await
        .expect("initialize should succeed");
    tokio::time::sleep(Duration::from_millis(2)).await;
    let tools = client
        .list_tools()
        .await
        .expect("idle timeout should reconnect before listing");

    assert_eq!(tools[0].name, "fresh");
    assert_eq!(
        client.last_reconnect_outcome(),
        Some(MCPRemoteReconnectOutcome::Recovered)
    );
    let attempted = attempted.lock().expect("protocol mutex should lock");
    assert_eq!(attempted.len(), 2);
}

#[tokio::test]
async fn list_tools_reconnects_on_periodic_policy_even_without_error() {
    let policy = MCPRemoteReconnectPolicy {
        periodic_reconnect_ms: Some(1),
        jitter_ratio: 0.0,
        ..MCPRemoteReconnectPolicy::default()
    };
    let transport = MockBootstrapTransport::new(vec![
        Ok(scripted_running_service(MockSessionBehavior::ListTools {
            tool_name: "first".into(),
        })),
        Ok(scripted_running_service(MockSessionBehavior::ListTools {
            tool_name: "second".into(),
        })),
    ])
    .with_reconnect_policy(policy);
    let attempted = transport.attempted_protocols();
    let mut client = MCPClient::new(Box::new(transport));

    client
        .initialize()
        .await
        .expect("initialize should succeed");
    let first = client.list_tools().await.expect("first list should work");
    tokio::time::sleep(Duration::from_millis(2)).await;
    let second = client
        .list_tools()
        .await
        .expect("periodic policy should reconnect before listing");

    assert_eq!(first[0].name, "first");
    assert_eq!(second[0].name, "second");
    let attempted = attempted.lock().expect("protocol mutex should lock");
    assert_eq!(attempted.len(), 2);
}

#[tokio::test]
async fn auth_failure_is_terminal_and_needs_auth() {
    let transport = MockBootstrapTransport::new(vec![
        Ok(scripted_running_service(
            MockSessionBehavior::AuthOnListTools,
        )),
        Ok(scripted_running_service(MockSessionBehavior::ListTools {
            tool_name: "should-not-retry".into(),
        })),
    ])
    .with_reconnect_policy(MCPRemoteReconnectPolicy {
        jitter_ratio: 0.0,
        ..MCPRemoteReconnectPolicy::default()
    });
    let attempted = transport.attempted_protocols();
    let mut client = MCPClient::new(Box::new(transport));

    client
        .initialize()
        .await
        .expect("initialize should succeed");
    let error = client
        .list_tools()
        .await
        .expect_err("auth failure should be terminal");

    assert!(matches!(error, RociError::Authentication(_)));
    assert_eq!(
        client.last_reconnect_outcome(),
        Some(MCPRemoteReconnectOutcome::NeedsAuth)
    );
    let attempted = attempted.lock().expect("protocol mutex should lock");
    assert_eq!(attempted.len(), 1);
}

#[tokio::test]
async fn exhausted_reconnect_attempts_surface_failed_outcome() {
    let transport = MockBootstrapTransport::new(vec![
        Ok(scripted_running_service(
            MockSessionBehavior::DisconnectOnListTools,
        )),
        Err(ClientInitializeError::ConnectionClosed("try 1".into())),
        Err(ClientInitializeError::ConnectionClosed("try 2".into())),
    ])
    .with_reconnect_policy(MCPRemoteReconnectPolicy {
        max_attempts: 2,
        initial_backoff_ms: 1,
        max_backoff_ms: 1,
        jitter_ratio: 0.0,
        ..MCPRemoteReconnectPolicy::default()
    });
    let attempted = transport.attempted_protocols();
    let mut client = MCPClient::new(Box::new(transport));

    client
        .initialize()
        .await
        .expect("initialize should succeed");
    let error = client
        .list_tools()
        .await
        .expect_err("exhausted reconnect should fail");

    assert!(error.to_string().contains("try 2"));
    assert_eq!(
        client.last_reconnect_outcome(),
        Some(MCPRemoteReconnectOutcome::Failed)
    );
    let attempted = attempted.lock().expect("protocol mutex should lock");
    assert_eq!(attempted.len(), 3);
}

#[test]
fn reconnect_predicate_treats_cancelled_as_transient() {
    let cancelled = ServiceError::Cancelled {
        reason: Some("transport dropped".into()),
    };
    assert!(MCPClient::should_reconnect_after_service_error(&cancelled));
}
