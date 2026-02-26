use super::*;
async fn no_panic_when_stream_optional_fields_missing() {
    let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let (sink, _events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert!(
        !result.messages.is_empty(),
        "completed runs should carry final conversation messages"
    );
    assert!(
        result
            .messages
            .iter()
            .any(|message| matches!(message.role, crate::types::Role::User)),
        "result should include persisted prompt context"
    );
}

#[tokio::test]

async fn request_transport_is_forwarded_to_provider_request() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.transport = Some(provider::TRANSPORT_PROXY.to_string());

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    assert!(
        !requests.is_empty(),
        "provider should receive at least one request"
    );
    assert_eq!(
        requests[0].transport.as_deref(),
        Some(provider::TRANSPORT_PROXY)
    );
}

#[tokio::test]
async fn unsupported_request_transport_is_rejected_before_provider_call() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.transport = Some("satellite".to_string());

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Failed);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("unsupported provider transport 'satellite'"),
        "expected unsupported transport error, got: {:?}",
        result.error
    );

    let requests = requests.lock().expect("request lock");
    assert!(
        requests.is_empty(),
        "provider should not be called for unsupported transports"
    );
}

#[tokio::test]
async fn convert_to_llm_hook_can_append_and_filter_custom_messages() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.convert_to_llm = Some(Arc::new(|mut messages: Vec<AgentMessage>| {
        Box::pin(async move {
            messages.push(AgentMessage::custom(
                "artifact",
                serde_json::json!({ "hidden": true }),
            ));
            messages.push(AgentMessage::user("hook-added"));
            convert_to_llm(&messages)
        })
    }));

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    assert!(!requests.is_empty(), "provider should receive one request");
    let first = &requests[0].messages;
    assert!(
        first.iter().any(|m| m.text() == "hook-added"),
        "conversion hook should be able to append LLM-visible messages"
    );
    assert!(
        first.iter().all(|m| matches!(
            m.role,
            crate::types::Role::System
                | crate::types::Role::User
                | crate::types::Role::Assistant
                | crate::types::Role::Tool
        )),
        "provider messages must remain LLM message roles after conversion"
    );
}

#[tokio::test]
async fn rate_limited_stream_retries_within_max_delay_cap() {
    let (runner, requests) = test_runner(ProviderScenario::RateLimitedThenComplete);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("retry")]);
    request.max_retry_delay_ms = Some(10);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
}

#[tokio::test]
async fn rate_limited_stream_fails_when_retry_delay_exceeds_cap() {
    let (runner, requests) = test_runner(ProviderScenario::RateLimitedExceedsCap);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("retry")]);
    request.max_retry_delay_ms = Some(10);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Failed);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("exceeds max_retry_delay_ms"),
        "expected max retry delay failure, got: {:?}",
        result.error
    );

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 1);
}

#[tokio::test]
async fn rate_limited_without_retry_hint_fails_immediately() {
    let (runner, requests) = test_runner(ProviderScenario::RateLimitedWithoutRetryHint);
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("retry")]);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Failed);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("without retry_after hint"),
        "expected missing retry hint failure, got: {:?}",
        result.error
    );

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 1);
}
