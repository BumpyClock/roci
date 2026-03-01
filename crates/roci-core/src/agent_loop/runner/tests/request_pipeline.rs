use super::*;

#[tokio::test]
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
    assert!(
        result.messages.iter().any(|message| {
            matches!(message.role, crate::types::Role::Assistant)
                && message
                    .content
                    .iter()
                    .any(|part| matches!(part, ContentPart::Text { text } if text == "done"))
        }),
        "tool-less turns should persist assistant text into run messages"
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
async fn provider_request_fields_are_forwarded_to_provider() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let callback_calls = std::sync::Arc::new(AtomicUsize::new(0));
    let callback_calls_for_hook = callback_calls.clone();

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "x-test-header",
        reqwest::header::HeaderValue::from_static("present"),
    );

    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.api_key_override = Some("sk-request-override".to_string());
    request.provider_headers = headers;
    request
        .provider_metadata
        .insert("trace_id".to_string(), "trace-123".to_string());
    request.provider_payload_callback = Some(std::sync::Arc::new(move |_payload| {
        callback_calls_for_hook.fetch_add(1, Ordering::SeqCst);
    }));

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
        requests[0].api_key_override.as_deref(),
        Some("sk-request-override")
    );
    assert_eq!(
        requests[0]
            .headers
            .get("x-test-header")
            .and_then(|value| value.to_str().ok()),
        Some("present")
    );
    assert_eq!(
        requests[0].metadata.get("trace_id").map(String::as_str),
        Some("trace-123")
    );
    assert!(
        requests[0].payload_callback.is_some(),
        "provider payload callback should be forwarded"
    );
    assert_eq!(
        callback_calls.load(Ordering::SeqCst),
        0,
        "stub provider does not invoke payload callback directly"
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
    request.convert_to_llm = Some(Arc::new(|mut payload| {
        Box::pin(async move {
            payload.messages.push(AgentMessage::custom(
                "artifact",
                serde_json::json!({ "hidden": true }),
            ));
            payload.messages.push(AgentMessage::user("hook-added"));
            Ok(ConvertToLlmHookResult::ReplaceMessages {
                messages: convert_to_llm(&payload.messages),
            })
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
async fn transform_context_runs_before_convert_to_llm() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let convert_seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let convert_seen_for_hook = convert_seen.clone();

    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.transform_context = Some(Arc::new(|_payload| {
        Box::pin(async move {
            Ok(TransformContextHookResult::ReplaceMessages {
                messages: vec![ModelMessage::user("from-transform")],
            })
        })
    }));
    request.convert_to_llm = Some(Arc::new(move |payload| {
        let convert_seen_for_hook = convert_seen_for_hook.clone();
        Box::pin(async move {
            let seen = payload
                .messages
                .iter()
                .map(|message| message.text().unwrap_or_default())
                .collect::<Vec<_>>();
            *convert_seen_for_hook.lock().expect("capture lock") = seen;
            Ok(ConvertToLlmHookResult::ReplaceMessages {
                messages: vec![ModelMessage::user("from-convert")],
            })
        })
    }));

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let seen = convert_seen.lock().expect("capture lock").clone();
    assert_eq!(seen, vec!["from-transform".to_string()]);

    let requests = requests.lock().expect("request lock");
    assert!(!requests.is_empty(), "provider should receive one request");
    let first = &requests[0].messages;
    assert!(
        first.iter().any(|m| m.text() == "from-convert"),
        "provider should receive converted payload after transform"
    );
    assert!(
        first.iter().all(|m| m.text() != "from-transform"),
        "transformed message should not bypass conversion replacement"
    );
}

#[tokio::test]
async fn transform_context_hook_cancel_fails_run_with_reason() {
    let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.transform_context = Some(Arc::new(|_payload| {
        Box::pin(async {
            Ok(TransformContextHookResult::Cancel {
                reason: Some("blocked by transform hook".to_string()),
            })
        })
    }));

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
            .contains("blocked by transform hook"),
        "expected transform cancel reason, got: {:?}",
        result.error
    );
}

#[tokio::test]
async fn abort_during_transform_context_cancels_hook_token_and_run() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let cancel_token_capture = Arc::new(std::sync::Mutex::new(None::<CancellationToken>));
    let cancel_token_capture_for_hook = cancel_token_capture.clone();

    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.transform_context = Some(Arc::new(move |payload| {
        let cancel_token_capture_for_hook = cancel_token_capture_for_hook.clone();
        Box::pin(async move {
            *cancel_token_capture_for_hook.lock().expect("capture lock") =
                Some(payload.cancellation_token.clone());
            payload.cancellation_token.cancelled().await;
            Ok(TransformContextHookResult::Continue)
        })
    }));

    let mut handle = runner.start(request).await.expect("start run");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(handle.abort(), "abort should be accepted");

    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Canceled);

    timeout(Duration::from_secs(2), async {
        loop {
            let maybe_token = cancel_token_capture.lock().expect("capture lock").clone();
            if let Some(token) = maybe_token {
                assert!(
                    token.is_cancelled(),
                    "transform hook cancellation token should be canceled"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("transform hook token should be canceled");

    let requests = requests.lock().expect("request lock");
    assert!(
        requests.is_empty(),
        "provider should not be called when canceling during transform"
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

#[tokio::test]
async fn retryable_timeout_retries_with_default_backoff_policy() {
    let (runner, requests) = test_runner(ProviderScenario::RetryableTimeoutThenComplete);
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("retry")]);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2, "default retry should perform one retry");
}

#[tokio::test]
async fn retryable_timeout_fails_after_max_attempts() {
    let (runner, requests) = test_runner(ProviderScenario::RetryableTimeoutExhausted);
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
            .contains("after 3 attempts"),
        "expected retry attempt exhaustion, got: {:?}",
        result.error
    );

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 3, "default max_attempts should be 3");
}

#[tokio::test]
async fn abort_during_retry_sleep_cancels_run() {
    let (runner, requests) = test_runner(ProviderScenario::RetryableTimeoutExhausted);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("retry")]);
    request.retry_backoff = RetryBackoffPolicy {
        max_attempts: 3,
        initial_delay_ms: 1_000,
        multiplier: 2.0,
        jitter_ratio: 0.0,
        max_delay_ms: 2_000,
    };

    let mut handle = runner.start(request).await.expect("start run");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(handle.abort(), "abort should be accepted");

    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Canceled);

    let requests = requests.lock().expect("request lock");
    assert_eq!(
        requests.len(),
        1,
        "run should cancel before retry sleep elapses into a second provider call"
    );
}

#[tokio::test]
async fn typed_overflow_error_triggers_compaction_recovery() {
    let (runner, requests) = test_runner(ProviderScenario::ContextOverflowThenComplete);
    let compaction_calls = std::sync::Arc::new(AtomicUsize::new(0));
    let compaction_calls_for_hook = compaction_calls.clone();

    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("overflow me")]);
    request.hooks = RunHooks {
        compaction: Some(std::sync::Arc::new(move |messages, _cancel| {
            let compaction_calls_for_hook = compaction_calls_for_hook.clone();
            Box::pin(async move {
                compaction_calls_for_hook.fetch_add(1, Ordering::SeqCst);
                let latest = messages
                    .last()
                    .cloned()
                    .unwrap_or_else(|| ModelMessage::user("overflow me"));
                Ok(Some(vec![
                    ModelMessage::user("<compaction_summary>trimmed</compaction_summary>"),
                    latest,
                ]))
            })
        })),
        pre_tool_use: None,
        post_tool_use: None,
    };

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(compaction_calls.load(Ordering::SeqCst), 1);

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|message| message.text().contains("<compaction_summary>")),
        "recovery request should include compacted context"
    );
}

#[tokio::test]
async fn untyped_overflow_error_does_not_trigger_compaction_recovery() {
    let (runner, requests) = test_runner(ProviderScenario::UntypedOverflowError);
    let compaction_calls = std::sync::Arc::new(AtomicUsize::new(0));
    let compaction_calls_for_hook = compaction_calls.clone();

    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("overflow me")]);
    request.hooks = RunHooks {
        compaction: Some(std::sync::Arc::new(move |_messages, _cancel| {
            let compaction_calls_for_hook = compaction_calls_for_hook.clone();
            Box::pin(async move {
                compaction_calls_for_hook.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            })
        })),
        pre_tool_use: None,
        post_tool_use: None,
    };

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Failed);
    assert_eq!(
        compaction_calls.load(Ordering::SeqCst),
        0,
        "overflow recovery must only trigger on typed error codes"
    );

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 1);
}

#[tokio::test]
async fn typed_overflow_fails_after_bounded_recovery_attempts() {
    let (runner, requests) = test_runner(ProviderScenario::ContextOverflowAlways);
    let compaction_calls = std::sync::Arc::new(AtomicUsize::new(0));
    let compaction_calls_for_hook = compaction_calls.clone();

    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("overflow me")]);
    request.hooks = RunHooks {
        compaction: Some(std::sync::Arc::new(move |_messages, _cancel| {
            let compaction_calls_for_hook = compaction_calls_for_hook.clone();
            Box::pin(async move {
                compaction_calls_for_hook.fetch_add(1, Ordering::SeqCst);
                Ok(Some(vec![ModelMessage::user(
                    "<compaction_summary>trimmed</compaction_summary>",
                )]))
            })
        })),
        pre_tool_use: None,
        post_tool_use: None,
    };

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
            .contains("persisted after 3 attempts"),
        "expected bounded overflow failure, got: {:?}",
        result.error
    );
    assert_eq!(
        compaction_calls.load(Ordering::SeqCst),
        2,
        "compaction should run for bounded recovery attempts before failure"
    );

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 3);
}

#[tokio::test]
async fn cancel_during_stream_preserves_latest_assistant_snapshot() {
    let (runner, _requests) = test_runner(ProviderScenario::PartialTextThenIdle);
    let mut handle = runner
        .start(RunRequest::new(
            test_model(),
            vec![ModelMessage::user("cancel this stream")],
        ))
        .await
        .expect("start run");

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(handle.abort(), "abort should be accepted");

    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Canceled);
    assert!(
        result.messages.iter().any(|message| {
            matches!(message.role, crate::types::Role::Assistant)
                && message
                    .content
                    .iter()
                    .any(|part| matches!(part, ContentPart::Text { text } if text == "partial"))
        }),
        "cancel should preserve latest assistant snapshot when available"
    );
}
