use super::*;

use crate::agent_loop::{ApprovalAction, ApprovalDecision, ApprovalMatcher, ApprovalRule};
use crate::tools::{ToolResultSizePolicy, ToolSafetyKind, ToolSafetyPlan, ToolSafetySummary};

fn read_only_safety_summary(kind: ToolSafetyKind) -> ToolSafetySummary {
    ToolSafetySummary {
        read_only_by_default: true,
        destructive_by_default: false,
        concurrency_safe_by_default: true,
        approval_kind: kind,
    }
}

fn approval_required_safety_summary(kind: ToolSafetyKind) -> ToolSafetySummary {
    ToolSafetySummary {
        read_only_by_default: false,
        destructive_by_default: false,
        concurrency_safe_by_default: false,
        approval_kind: kind,
    }
}

fn command_safety_from_args(args: &ToolArguments) -> ToolSafetyPlan {
    let command = args.get_str("command").unwrap_or("");
    ToolSafetyPlan::from_command_insight(crate::security::command::classify_shell_command(command))
}

fn tracked_safe_success_tool(
    name: &str,
    delay: Duration,
    active_calls: Arc<AtomicUsize>,
    max_active_calls: Arc<AtomicUsize>,
) -> Arc<dyn Tool> {
    let tool_name = name.to_string();
    Arc::new(
        AgentTool::new(
            tool_name.clone(),
            format!("{tool_name} tool"),
            AgentToolParameters::empty(),
            move |_args, _ctx: ToolExecutionContext| {
                let tool_name = tool_name.clone();
                let active_calls = active_calls.clone();
                let max_active_calls = max_active_calls.clone();
                async move {
                    let active_now = active_calls.fetch_add(1, Ordering::SeqCst) + 1;
                    let mut observed_max = max_active_calls.load(Ordering::SeqCst);
                    while active_now > observed_max {
                        match max_active_calls.compare_exchange(
                            observed_max,
                            active_now,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        ) {
                            Ok(_) => break,
                            Err(next) => observed_max = next,
                        }
                    }
                    tokio::time::sleep(delay).await;
                    active_calls.fetch_sub(1, Ordering::SeqCst);
                    Ok(serde_json::json!({ "tool": tool_name }))
                }
            },
        )
        .with_static_safety(
            ToolSafetyPlan::safe_read_only(ToolSafetyKind::Read),
            read_only_safety_summary(ToolSafetyKind::Read),
        ),
    )
}

#[tokio::test]
async fn tool_with_schema_rejects_bad_args_through_runner() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolBadArgs);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![schema_tool()];
    request.approval_policy = ApprovalPolicy::always();
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
    request.approval_policy = ApprovalPolicy::always();
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
    request.approval_policy = ApprovalPolicy::always();
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
    request.approval_policy = ApprovalPolicy::always();
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
    request.approval_policy = ApprovalPolicy::always();
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
async fn invalid_finalized_tool_args_skip_approval_and_execution() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let approval_requests = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![tracked_schema_path_tool(executions.clone())];
    request.approval_policy = ApprovalPolicy::ask();
    request.approval_handler = Some(Arc::new({
        let approval_requests = approval_requests.clone();
        move |_request| {
            let approval_requests = approval_requests.clone();
            Box::pin(async move {
                approval_requests.fetch_add(1, Ordering::SeqCst);
                ApprovalDecision::Accept
            })
        }
    }));
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: Some(Arc::new(|_call, _cancel| {
            Box::pin(async {
                Ok(PreToolUseHookResult::ReplaceArgs {
                    args: serde_json::json!({}),
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
    assert_eq!(approval_requests.load(Ordering::SeqCst), 0);

    let events = events.lock().expect("event lock");
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.payload, RunEventPayload::ApprovalRequired { .. })),
        "invalid finalized args must skip approval"
    );
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (call_id, result_json, is_error) = &tool_results[0];
    assert_eq!(call_id, "schema-call-1");
    assert!(*is_error, "validation failure must set is_error: true");
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
async fn pre_tool_use_hook_replace_args_are_used_by_approval() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![Arc::new(
        AgentTool::new(
            "schema_tool",
            "tool with required command param",
            AgentToolParameters::object()
                .string("command", "shell command", true)
                .build(),
            {
                let executions = executions.clone();
                move |args, _ctx: ToolExecutionContext| {
                    let executions = executions.clone();
                    async move {
                        executions.fetch_add(1, Ordering::SeqCst);
                        Ok(serde_json::json!({
                            "command": args.get_str("command")?,
                        }))
                    }
                }
            },
        )
        .with_safety(
            approval_required_safety_summary(ToolSafetyKind::CommandExecution),
            command_safety_from_args,
        ),
    )];
    request.approval_policy = ApprovalPolicy {
        default_action: ApprovalAction::Allow,
        rules: vec![ApprovalRule::new(
            "rewritten-command",
            ApprovalAction::Ask,
            ApprovalMatcher::CommandPattern {
                pattern: "approval-rewritten".to_string(),
            },
        )],
        additional_safety_floors: Default::default(),
        session_grants: Default::default(),
    };
    request.approval_handler = Some(Arc::new(|_request| {
        Box::pin(async { ApprovalDecision::Accept })
    }));
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: Some(Arc::new(|_call, _cancel| {
            Box::pin(async {
                Ok(PreToolUseHookResult::ReplaceArgs {
                    args: serde_json::json!({ "command": "echo approval-rewritten" }),
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
    let approval_requests = events
        .iter()
        .filter_map(|event| match &event.payload {
            RunEventPayload::ApprovalRequired { request } => Some(request),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(approval_requests.len(), 1);
    let payload = &approval_requests[0].payload;
    assert_eq!(
        payload.pointer("/evaluation/action"),
        Some(&serde_json::json!("ask"))
    );
    assert_eq!(
        payload.pointer("/evaluation/matched_rules/0/rule_id"),
        Some(&serde_json::json!("rewritten-command"))
    );
    assert!(payload.get("arguments").is_none());

    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (_call_id, result_json, is_error) = &tool_results[0];
    assert!(!is_error, "approved rewritten args should execute");
    assert_eq!(
        result_json["command"],
        serde_json::json!("echo approval-rewritten")
    );
}

#[tokio::test]
async fn pre_tool_use_hook_rewritten_args_can_fall_through_approval_default() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![Arc::new(
        AgentTool::new(
            "schema_tool",
            "tool with required command param",
            AgentToolParameters::object()
                .string("command", "shell command", true)
                .build(),
            {
                let executions = executions.clone();
                move |args, _ctx: ToolExecutionContext| {
                    let executions = executions.clone();
                    async move {
                        executions.fetch_add(1, Ordering::SeqCst);
                        Ok(serde_json::json!({
                            "command": args.get_str("command")?,
                        }))
                    }
                }
            },
        )
        .with_safety(
            approval_required_safety_summary(ToolSafetyKind::CommandExecution),
            command_safety_from_args,
        ),
    )];
    request.approval_policy = ApprovalPolicy {
        default_action: ApprovalAction::Allow,
        rules: vec![ApprovalRule::new(
            "rewritten-command",
            ApprovalAction::Ask,
            ApprovalMatcher::CommandPattern {
                pattern: "approval-rewritten".to_string(),
            },
        )],
        additional_safety_floors: Default::default(),
        session_grants: Default::default(),
    };
    request.approval_handler = Some(Arc::new(|_request| {
        Box::pin(async { ApprovalDecision::Decline })
    }));
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: Some(Arc::new(|_call, _cancel| {
            Box::pin(async {
                Ok(PreToolUseHookResult::ReplaceArgs {
                    args: serde_json::json!({ "command": "echo approval-not-matching" }),
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
    let approval_requests = events
        .iter()
        .filter(|event| matches!(event.payload, RunEventPayload::ApprovalRequired { .. }))
        .count();
    assert_eq!(approval_requests, 0);

    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (_call_id, result_json, is_error) = &tool_results[0];
    assert!(!is_error, "default-allowed rewritten args should execute");
    assert_eq!(
        result_json["command"],
        serde_json::json!("echo approval-not-matching")
    );
}

#[tokio::test]
async fn pre_tool_use_hook_error_returns_synthetic_tool_error() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![tracked_schema_path_tool(executions.clone())];
    request.approval_policy = ApprovalPolicy::always();
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
async fn pre_tool_use_block_result_survives_parallel_flush_and_steering() {
    let (runner, _requests) = test_runner(ProviderScenario::ParallelSafeBatchThenComplete);
    let (sink, events) = capture_events();
    let active_calls = Arc::new(AtomicUsize::new(0));
    let max_active_calls = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("run tools")]);
    request.tools = vec![
        tracked_safe_success_tool(
            "read",
            Duration::from_millis(10),
            active_calls.clone(),
            max_active_calls.clone(),
        ),
        tracked_safe_success_tool(
            "ls",
            Duration::from_millis(10),
            active_calls,
            max_active_calls,
        ),
    ];
    request.approval_policy = ApprovalPolicy::always();
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: Some(Arc::new(|call, _cancel| {
            Box::pin(async move {
                if call.name == "ls" {
                    Ok(PreToolUseHookResult::Block {
                        reason: Some("blocked-before-steering".to_string()),
                    })
                } else {
                    Ok(PreToolUseHookResult::Continue)
                }
            })
        })),
        post_tool_use: None,
    };
    request.get_steering_messages = Some(Arc::new(|| {
        Box::pin(async { vec![ModelMessage::user("interrupt")] })
    }));

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(4), handle.wait())
        .await
        .expect("run should complete without timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 2);
    assert!(tool_results
        .iter()
        .any(|(call_id, _, is_error)| { call_id == "safe-read-1" && !is_error }));
    let (_call_id, result_json, is_error) = tool_results
        .iter()
        .find(|(call_id, _, _)| call_id == "safe-ls-2")
        .expect("pre-tool block result should be preserved for current call");
    assert!(*is_error);
    assert_eq!(result_json["source"], serde_json::json!("pre_tool_use"));
    assert!(result_json["error"]
        .as_str()
        .unwrap_or_default()
        .contains("blocked-before-steering"));
}

#[tokio::test]
async fn post_tool_use_hook_can_mutate_tool_result() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![tracked_schema_path_tool(executions.clone())];
    request.approval_policy = ApprovalPolicy::always();
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
async fn post_tool_use_hook_can_mutate_tool_result_size_is_capped() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![Arc::new(
        AgentTool::new(
            "schema_tool",
            "tool with required path param",
            AgentToolParameters::object()
                .string("path", "file path", true)
                .build(),
            {
                let executions = executions.clone();
                move |args, _ctx: ToolExecutionContext| {
                    let executions = executions.clone();
                    async move {
                        executions.fetch_add(1, Ordering::SeqCst);
                        Ok(serde_json::json!({
                            "path": args.get_str("path")?,
                        }))
                    }
                }
            },
        )
        .with_result_policy(ToolResultSizePolicy {
            max_result_size_bytes: Some(360),
        }),
    )];
    request.approval_policy = ApprovalPolicy::always();
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: None,
        post_tool_use: Some(Arc::new(|_call, mut result| {
            Box::pin(async move {
                if let Some(map) = result.result.as_object_mut() {
                    map.insert("post_mutated".to_string(), serde_json::json!(true));
                    map.insert("expanded".to_string(), serde_json::json!("z".repeat(600)));
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
    assert_eq!(result_json["truncated"], serde_json::json!(true));
    assert_eq!(
        result_json["reason"],
        serde_json::json!("tool_result_size_limit_exceeded")
    );
    let preview = result_json["preview"].as_str().unwrap_or_default();
    assert!(
        preview.contains("post_mutated") || preview.contains("expanded"),
        "preview should come from post-hook-expanded result"
    );
}

#[tokio::test]
async fn tool_result_size_caps_validation_error_after_post_hook_growth() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolBadArgs);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![
            Arc::new(
                AgentTool::new(
                    "schema_tool",
                    "tool with required path param",
                    AgentToolParameters::object()
                        .string("path", "file path", true)
                        .build(),
                    |_args, _ctx: ToolExecutionContext| async move {
                        Ok(serde_json::json!({ "ok": true }))
                    },
                )
                .with_result_policy(ToolResultSizePolicy {
                    max_result_size_bytes: Some(150),
                }),
            ),
        ];
    request.approval_policy = ApprovalPolicy::always();
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: None,
        post_tool_use: Some(Arc::new(|_call, mut result| {
            Box::pin(async move {
                if let Some(map) = result.result.as_object_mut() {
                    map.insert("expanded".to_string(), serde_json::json!("z".repeat(600)));
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

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (_call_id, result_json, is_error) = &tool_results[0];
    assert!(*is_error);
    assert_eq!(result_json["truncated"], serde_json::json!(true));
    assert_eq!(
        result_json["reason"],
        serde_json::json!("tool_result_size_limit_exceeded")
    );
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
        tracked_success_tool(
            "ls",
            Duration::from_millis(40),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        ),
    ];
    request.approval_policy = ApprovalPolicy::always();
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
    request.approval_policy = ApprovalPolicy::always();
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
