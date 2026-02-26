use super::*;
async fn tool_with_schema_rejects_bad_args_through_runner() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolBadArgs);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![schema_tool()];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");

    // The run must not panic and should complete (provider returns text-only on call 1).
    assert_eq!(result.status, RunStatus::Completed);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (call_id, result_json, is_error) = &tool_results[0];
    assert_eq!(call_id, "schema-call-1");
    assert!(is_error, "validation failure must set is_error: true");
    let error_msg = result_json["error"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("Argument validation failed"),
        "expected validation error prefix, got: {error_msg}"
    );
    assert!(
        error_msg.contains("missing required field 'path'"),
        "expected missing field detail, got: {error_msg}"
    );
}

#[tokio::test]
async fn tool_with_schema_accepts_valid_args_through_runner() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![schema_tool()];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");

    assert_eq!(result.status, RunStatus::Completed);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (call_id, result_json, is_error) = &tool_results[0];
    assert_eq!(call_id, "schema-call-1");
    assert!(!is_error, "valid args must not set is_error");
    assert_eq!(
        result_json["ok"], true,
        "tool handler should execute and return ok"
    );
}

#[tokio::test]
async fn tool_with_type_mismatch_rejects_through_runner() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolTypeMismatch);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![schema_tool()];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");

    assert_eq!(result.status, RunStatus::Completed);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (call_id, result_json, is_error) = &tool_results[0];
    assert_eq!(call_id, "schema-call-1");
    assert!(is_error, "type mismatch must set is_error: true");
    let error_msg = result_json["error"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("Argument validation failed"),
        "expected validation error prefix, got: {error_msg}"
    );
    assert!(
        error_msg.contains("expected type 'string'"),
        "expected type mismatch detail, got: {error_msg}"
    );
}

#[tokio::test]
async fn pre_tool_use_hook_can_block_and_skip_tool_execution() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![tracked_schema_path_tool(executions.clone())];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: Some(Arc::new(|_call, _cancel| {
            Box::pin(async {
                Ok(PreToolUseHookResult::Block {
                    reason: Some("blocked-by-test".to_string()),
                })
            })
        })),
        post_tool_use: None,
    };

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(executions.load(Ordering::SeqCst), 0);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (_call_id, result_json, is_error) = &tool_results[0];
    assert!(*is_error, "blocked call must be an error");
    assert_eq!(result_json["source"], serde_json::json!("pre_tool_use"));
    assert!(
        result_json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("blocked-by-test"),
        "blocked reason should be surfaced"
    );
}

#[tokio::test]
async fn pre_tool_use_hook_replace_args_are_used_by_tool() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![tracked_schema_path_tool(executions.clone())];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: Some(Arc::new(|_call, _cancel| {
            Box::pin(async {
                Ok(PreToolUseHookResult::ReplaceArgs {
                    args: serde_json::json!({ "path": "/tmp/replaced-by-hook" }),
                })
            })
        })),
        post_tool_use: None,
    };

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (_call_id, result_json, is_error) = &tool_results[0];
    assert!(!is_error, "hook-rewritten args should still be valid");
    assert_eq!(
        result_json["path"],
        serde_json::json!("/tmp/replaced-by-hook")
    );
}

#[tokio::test]
async fn pre_tool_use_hook_error_returns_synthetic_tool_error() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![tracked_schema_path_tool(executions.clone())];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: Some(Arc::new(|_call, _cancel| {
            Box::pin(async {
                Err(RociError::InvalidState(
                    "forced pre hook failure".to_string(),
                ))
            })
        })),
        post_tool_use: None,
    };

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(executions.load(Ordering::SeqCst), 0);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (_call_id, result_json, is_error) = &tool_results[0];
    assert!(*is_error, "pre-hook errors must become tool errors");
    assert_eq!(result_json["source"], serde_json::json!("pre_tool_use"));
    assert!(result_json["error"]
        .as_str()
        .unwrap_or_default()
        .contains("forced pre hook failure"));
}

#[tokio::test]
async fn post_tool_use_hook_can_mutate_tool_result() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![tracked_schema_path_tool(executions.clone())];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: None,
        post_tool_use: Some(Arc::new(|_call, mut result| {
            Box::pin(async move {
                if let Some(map) = result.result.as_object_mut() {
                    map.insert("post_mutated".to_string(), serde_json::json!(true));
                }
                Ok(result)
            })
        })),
    };

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (_call_id, result_json, is_error) = &tool_results[0];
    assert!(!is_error);
    assert_eq!(result_json["post_mutated"], serde_json::json!(true));
}

#[tokio::test]
async fn post_tool_use_hook_runs_for_skipped_synthetic_errors() {
    let (runner, _requests) = test_runner(ProviderScenario::MutatingBatchThenComplete);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("run tools")]);
    request.tools = vec![
        tracked_success_tool(
            "apply_patch",
            Duration::from_millis(40),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        ),
        tracked_success_tool(
            "read",
            Duration::from_millis(40),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        ),
    ];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);
    let seen_calls = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen_calls_for_hook = seen_calls.clone();
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: None,
        post_tool_use: Some(Arc::new(move |call, mut result| {
            let seen_calls_for_hook = seen_calls_for_hook.clone();
            Box::pin(async move {
                seen_calls_for_hook
                    .lock()
                    .expect("seen calls lock")
                    .push(call.id.clone());
                if let Some(map) = result.result.as_object_mut() {
                    map.insert("post_seen".to_string(), serde_json::json!(true));
                }
                Ok(result)
            })
        })),
    };
    let steering_tick = Arc::new(AtomicUsize::new(0));
    let steering_tick_clone = steering_tick.clone();
    request.get_steering_messages = Some(Arc::new(move || {
        let tick = steering_tick_clone.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            if tick >= 2 {
                vec![ModelMessage::user("interrupt")]
            } else {
                Vec::new()
            }
        })
    }));

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(4), handle.wait())
        .await
        .expect("run should complete without timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let seen_calls = seen_calls.lock().expect("seen calls lock");
    assert!(
        seen_calls.iter().any(|id| id == "safe-read-2"),
        "post hook should run for skipped synthetic result"
    );
    drop(seen_calls);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    let (_call_id, result_json, is_error) = tool_results
        .iter()
        .find(|(call_id, _, _)| call_id == "safe-read-2")
        .expect("expected skipped safe-read-2 result");
    assert!(*is_error);
    assert_eq!(result_json["post_seen"], serde_json::json!(true));
}

#[tokio::test]
async fn post_tool_use_hook_error_returns_deterministic_synthetic_error() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![tracked_schema_path_tool(executions.clone())];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: None,
        post_tool_use: Some(Arc::new(|_call, _result| {
            Box::pin(async {
                Err(RociError::InvalidState(
                    "forced post hook failure".to_string(),
                ))
            })
        })),
    };

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (_call_id, result_json, is_error) = &tool_results[0];
    assert!(*is_error);
    assert_eq!(result_json["source"], serde_json::json!("post_tool_use"));
    assert_eq!(
        result_json["original_result"]["path"],
        serde_json::json!("/tmp/test")
    );
    assert!(result_json["error"]
        .as_str()
        .unwrap_or_default()
        .contains("forced post hook failure"));
}
