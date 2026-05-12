use super::*;

use crate::tools::ToolPromptMetadata;

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
async fn prompt_metadata_schema_uses_prompt_and_renders_available_tools_transiently() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let mut request = RunRequest::new(
        test_model(),
        vec![
            ModelMessage::system("root system"),
            ModelMessage::user("use tools"),
        ],
    );
    request.tools = vec![
            Arc::new(
                AgentTool::new(
                    "inspect",
                    "UI catalog description",
                    AgentToolParameters::empty(),
                    |_args, _ctx: ToolExecutionContext| async move {
                        Ok(serde_json::json!({ "ok": true }))
                    },
                )
                .with_prompt("Model-facing inspect prompt")
                .with_prompt_metadata(ToolPromptMetadata {
                    guidelines: vec!["Use when exact inspection is needed.".to_string()],
                    search_hint: Some("never-render-this-search-hint".to_string()),
                }),
            ),
        ];

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    let first = requests.first().expect("provider request");
    let tools = first.tools.as_ref().expect("provider tools");
    assert_eq!(tools[0].description, "Model-facing inspect prompt");

    assert_eq!(first.messages[0].text(), "root system");
    assert_eq!(first.messages[1].role, crate::types::Role::System);
    let metadata = first.messages[1].text();
    assert!(metadata.contains("<available_tools>"));
    assert!(metadata.contains("<tool name=\"inspect\">"));
    assert!(metadata.contains("<prompt>Model-facing inspect prompt</prompt>"));
    assert!(metadata.contains("- Use when exact inspection is needed."));
    assert!(!metadata.contains("never-render-this-search-hint"));
    assert_eq!(first.messages[2].text(), "use tools");

    assert!(
        result
            .messages
            .iter()
            .all(|message| !message.text().contains("<available_tools>")),
        "tool metadata must not persist into run messages"
    );
}

#[tokio::test]
async fn prompt_metadata_is_deterministic_and_skips_default_schema_duplicates() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("use tools")]);
    request.tools = vec![
        Arc::new(AgentTool::new(
            "default_only",
            "default description",
            AgentToolParameters::empty(),
            |_args, _ctx: ToolExecutionContext| async move { Ok(serde_json::json!({})) },
        )),
        Arc::new(
            AgentTool::new(
                "first",
                "first description",
                AgentToolParameters::empty(),
                |_args, _ctx: ToolExecutionContext| async move { Ok(serde_json::json!({})) },
            )
            .with_prompt_metadata(ToolPromptMetadata {
                guidelines: vec!["First guideline".to_string()],
                search_hint: None,
            }),
        ),
        Arc::new(
            AgentTool::new(
                "second",
                "second description",
                AgentToolParameters::empty(),
                |_args, _ctx: ToolExecutionContext| async move { Ok(serde_json::json!({})) },
            )
            .with_prompt("second prompt")
            .with_prompt_metadata(ToolPromptMetadata {
                guidelines: vec!["Second guideline".to_string()],
                search_hint: Some("hidden-search-hint".to_string()),
            }),
        ),
    ];

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    let first = requests.first().expect("provider request");
    let metadata = first
        .messages
        .iter()
        .find(|message| message.text().contains("<available_tools>"))
        .expect("available tools metadata")
        .text();
    assert_eq!(
        metadata,
        "<available_tools>\n\
<tool name=\"first\">\n\
<prompt>first description</prompt>\n\
<guidelines>\n\
- First guideline\n\
</guidelines>\n\
</tool>\n\
<tool name=\"second\">\n\
<prompt>second prompt</prompt>\n\
<guidelines>\n\
- Second guideline\n\
</guidelines>\n\
</tool>\n\
</available_tools>"
    );
    assert!(!metadata.contains("default_only"));
    assert!(!metadata.contains("hidden-search-hint"));
}

#[tokio::test]
async fn prompt_metadata_renders_custom_prompt_without_guidelines() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("use tools")]);
    request.tools = vec![Arc::new(
        AgentTool::new(
            "summarize",
            "UI summary",
            AgentToolParameters::empty(),
            |_args, _ctx: ToolExecutionContext| async move { Ok(serde_json::json!({})) },
        )
        .with_prompt("Model summary prompt"),
    )];

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    let metadata = requests[0]
        .messages
        .iter()
        .find(|message| message.text().contains("<available_tools>"))
        .expect("available tools metadata")
        .text();
    assert_eq!(
        metadata,
        "<available_tools>\n\
<tool name=\"summarize\">\n\
<prompt>Model summary prompt</prompt>\n\
</tool>\n\
</available_tools>"
    );
    assert!(!metadata.contains("<guidelines>"));
}

#[tokio::test]
async fn prompt_metadata_search_hint_only_does_not_render_available_tools() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("use tools")]);
    request.tools = vec![Arc::new(
        AgentTool::new(
            "search",
            "Search workspace",
            AgentToolParameters::empty(),
            |_args, _ctx: ToolExecutionContext| async move { Ok(serde_json::json!({})) },
        )
        .with_prompt_metadata(ToolPromptMetadata {
            guidelines: Vec::new(),
            search_hint: Some("future-search-only".to_string()),
        }),
    )];

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    let provider_request = requests.first().expect("provider request");
    assert!(
        provider_request
            .messages
            .iter()
            .all(|message| !message.text().contains("<available_tools>")),
        "search_hint alone should not render available_tools metadata"
    );
}

#[tokio::test]
async fn alias_historical_tool_calls_are_normalized_only_in_provider_payload() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let historical_alias_call = AgentToolCall {
        id: "historical-call-1".to_string(),
        name: "old_read".to_string(),
        arguments: serde_json::json!({ "path": "README.md" }),
        called_as: None,
        recipient: None,
    };
    let mut request = RunRequest::new(
        test_model(),
        vec![
            ModelMessage::user("previous request"),
            ModelMessage {
                role: crate::types::Role::Assistant,
                content: vec![ContentPart::ToolCall(historical_alias_call.clone())],
                name: None,
                timestamp: None,
                metadata: None,
            },
        ],
    );
    request.tools = vec![Arc::new(
        AgentTool::new(
            "read_file",
            "read file",
            AgentToolParameters::empty(),
            |_args, _ctx: ToolExecutionContext| async move { Ok(serde_json::json!({})) },
        )
        .with_aliases(["old_read"]),
    )];

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Completed);
    let requests = requests.lock().expect("request lock");
    let provider_call = requests[0]
        .messages
        .iter()
        .flat_map(|message| message.tool_calls())
        .next()
        .expect("provider-facing tool call");
    assert_eq!(provider_call.name, "read_file");
    assert_eq!(provider_call.called_as.as_deref(), Some("old_read"));

    let persisted_call = result
        .messages
        .iter()
        .flat_map(|message| message.tool_calls())
        .next()
        .expect("persisted historical tool call");
    assert_eq!(persisted_call, &historical_alias_call);
}

#[tokio::test]
async fn alias_historical_canonical_self_alias_does_not_set_called_as() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let historical_canonical_call = AgentToolCall {
        id: "historical-call-1".to_string(),
        name: "read_file".to_string(),
        arguments: serde_json::json!({ "path": "README.md" }),
        called_as: None,
        recipient: None,
    };
    let mut request = RunRequest::new(
        test_model(),
        vec![ModelMessage {
            role: crate::types::Role::Assistant,
            content: vec![ContentPart::ToolCall(historical_canonical_call.clone())],
            name: None,
            timestamp: None,
            metadata: None,
        }],
    );
    request.tools = vec![Arc::new(
        AgentTool::new(
            "read_file",
            "read file",
            AgentToolParameters::empty(),
            |_args, _ctx: ToolExecutionContext| async move { Ok(serde_json::json!({})) },
        )
        .with_aliases(["read_file"]),
    )];

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Completed);
    let requests = requests.lock().expect("request lock");
    let provider_call = requests[0]
        .messages
        .iter()
        .flat_map(|message| message.tool_calls())
        .next()
        .expect("provider-facing tool call");
    assert_eq!(provider_call.name, "read_file");
    assert_eq!(provider_call.called_as, None);
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
async fn fallback_resolves_api_key_for_active_provider() {
    let (runner, requests) = test_runner_by_model(vec![
        ("openai-model", ProviderScenario::RateLimitedExceedsCap),
        ("anthropic-model", ProviderScenario::MissingOptionalFields),
    ]);
    let mut request = RunRequest::with_candidates(
        vec![
            LanguageModel::Custom {
                provider: "openai".to_string(),
                model_id: "openai-model".to_string(),
            },
            LanguageModel::Custom {
                provider: "anthropic".to_string(),
                model_id: "anthropic-model".to_string(),
            },
        ],
        vec![ModelMessage::user("hello")],
    )
    .expect("valid candidates");
    request.retry_mode = RetryMode::Bounded { max_attempts: 1 };
    request.get_api_key = Some(Arc::new(|model| {
        Box::pin(async move {
            Ok(match model.provider_name() {
                "openai" => "sk-openai".to_string(),
                "anthropic" => "sk-anthropic".to_string(),
                other => panic!("unexpected provider {other}"),
            })
        })
    }));

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].api_key_override.as_deref(), Some("sk-openai"));
    assert_eq!(
        requests[1].api_key_override.as_deref(),
        Some("sk-anthropic")
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
async fn rate_limited_without_retry_hint_uses_bounded_backoff() {
    let (runner, requests) = test_runner(ProviderScenario::RateLimitedWithoutRetryHint);
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("retry")])
        .with_retry_backoff(RetryBackoffPolicy {
            max_attempts: 2,
            initial_delay_ms: 1,
            multiplier: 1.0,
            jitter_ratio: 0.0,
            max_delay_ms: 1,
        });

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
            .contains("retry budget exhausted"),
        "expected retry budget failure, got: {:?}",
        result.error
    );

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
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
    // With the progress-gated policy, the trivial compaction hook does not
    // free enough tokens (< 500) to justify a second compaction. The policy
    // aborts after the first compaction with CompactionProgressInsufficient.
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("insufficient progress"),
        "expected progress-gated abort, got: {:?}",
        result.error
    );
    assert_eq!(
        compaction_calls.load(Ordering::SeqCst),
        1,
        "trivial compaction frees < 500 tokens; second compaction should not run"
    );

    let requests = requests.lock().expect("request lock");
    assert_eq!(
        requests.len(),
        2,
        "initial call + one retry after compaction"
    );
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
