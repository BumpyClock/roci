use super::*;

use crate::session::{LocalSessionFs, LogicalPath, SessionFs};
use crate::tools::SandboxProvider;
use crate::tools::{ToolSafetyKind, ToolSafetyPlan, ToolSafetySummary};

#[derive(Debug, Default)]
struct RecordingSandboxProvider {
    calls: std::sync::Mutex<Vec<(String, LogicalPath)>>,
}

fn read_only_safety_summary(kind: ToolSafetyKind) -> ToolSafetySummary {
    ToolSafetySummary {
        read_only_by_default: true,
        destructive_by_default: false,
        concurrency_safe_by_default: true,
        approval_kind: kind,
    }
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

#[async_trait]
impl SandboxProvider for RecordingSandboxProvider {
    async fn validate_shell_command(
        &self,
        command: &str,
        cwd: &LogicalPath,
    ) -> Result<(), RociError> {
        self.calls
            .lock()
            .expect("sandbox calls lock")
            .push((command.to_string(), cwd.clone()));
        Ok(())
    }
}

#[tokio::test]
async fn run_request_threads_session_context_to_tools() {
    let (runner, _requests) = test_runner(ProviderScenario::ToolCallWithUsageThenTextWithUsage);
    let temp = tempfile::tempdir().expect("temp dir should be created");
    let session_fs: Arc<dyn SessionFs + Send + Sync> =
        Arc::new(LocalSessionFs::new(temp.path().join("session")).expect("session fs"));
    let session_cwd = LogicalPath::parse("work").expect("logical path");
    let observed_context = Arc::new(std::sync::Mutex::new(Vec::<(Option<String>, bool)>::new()));
    let tool_observed_context = observed_context.clone();
    let noop_tool: Arc<dyn Tool> = Arc::new(AgentTool::new(
        "noop_tool",
        "records tool context",
        AgentToolParameters::empty(),
        move |_args, ctx: ToolExecutionContext| {
            let tool_observed_context = tool_observed_context.clone();
            async move {
                tool_observed_context.lock().expect("context lock").push((
                    ctx.session_cwd.as_ref().map(|cwd| cwd.as_str().to_string()),
                    ctx.session_fs.is_some(),
                ));
                Ok(serde_json::json!({ "ok": true }))
            }
        },
    ));
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("run tool")])
        .with_tools(vec![noop_tool])
        .with_approval_policy(ApprovalPolicy::always())
        .with_session_context(session_fs, session_cwd);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Completed);
    let contexts = observed_context.lock().expect("context lock");
    assert_eq!(contexts.as_slice(), &[(Some("work".to_string()), true)]);
}

#[tokio::test]
async fn run_request_threads_sandbox_provider_to_tools() {
    let (runner, _requests) = test_runner(ProviderScenario::ToolCallWithUsageThenTextWithUsage);
    let sandbox_provider = Arc::new(RecordingSandboxProvider::default());
    let observed_context = Arc::new(std::sync::Mutex::new(Vec::<(bool, Option<String>)>::new()));
    let tool_observed_context = observed_context.clone();
    let noop_tool: Arc<dyn Tool> = Arc::new(AgentTool::new(
        "noop_tool",
        "records sandbox context",
        AgentToolParameters::empty(),
        move |_args, ctx: ToolExecutionContext| {
            let tool_observed_context = tool_observed_context.clone();
            async move {
                let cwd = ctx.session_cwd.unwrap_or_else(LogicalPath::root);
                if let Some(provider) = ctx.sandbox_provider.as_ref() {
                    provider.validate_shell_command("echo ok", &cwd).await?;
                }
                tool_observed_context.lock().expect("context lock").push((
                    ctx.sandbox_provider.is_some(),
                    Some(cwd.as_str().to_string()),
                ));
                Ok(serde_json::json!({ "ok": true }))
            }
        },
    ));
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("run tool")])
        .with_tools(vec![noop_tool])
        .with_approval_policy(ApprovalPolicy::always())
        .with_sandbox_provider(sandbox_provider.clone());

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Completed);
    let contexts = observed_context.lock().expect("context lock");
    assert_eq!(contexts.as_slice(), &[(true, Some("".to_string()))]);
    let sandbox_calls = sandbox_provider.calls.lock().expect("sandbox calls lock");
    assert_eq!(
        sandbox_calls.as_slice(),
        &[("echo ok".to_string(), LogicalPath::root())]
    );
}

#[tokio::test]
async fn tool_visibility_policy_filters_provider_tool_definitions() {
    let (runner, requests) = test_runner(ProviderScenario::TextOnlyWithUsage);
    let active_calls = Arc::new(AtomicUsize::new(0));
    let max_active_calls = Arc::new(AtomicUsize::new(0));
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")])
        .with_tools(vec![
            tracked_success_tool(
                "read",
                Duration::from_millis(0),
                active_calls.clone(),
                max_active_calls.clone(),
            ),
            tracked_success_tool(
                "write",
                Duration::from_millis(0),
                active_calls,
                max_active_calls,
            ),
        ])
        .with_tool_visibility_policy(ToolVisibilityPolicy::allow_only(["read"]))
        .with_approval_policy(ApprovalPolicy::always());

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Completed);
    let requests = requests.lock().expect("request lock");
    let tools = requests[0].tools.as_ref().expect("provider tools");
    let tool_names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(tool_names, vec!["read"]);
}

#[tokio::test]
async fn tool_visibility_policy_can_hide_all_provider_tools() {
    let (runner, requests) = test_runner(ProviderScenario::TextOnlyWithUsage);
    let active_calls = Arc::new(AtomicUsize::new(0));
    let max_active_calls = Arc::new(AtomicUsize::new(0));
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")])
        .with_tools(vec![tracked_success_tool(
            "read",
            Duration::from_millis(0),
            active_calls,
            max_active_calls,
        )])
        .with_tool_visibility_policy(ToolVisibilityPolicy::no_tools())
        .with_approval_policy(ApprovalPolicy::always());

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Completed);
    let requests = requests.lock().expect("request lock");
    assert!(requests[0].tools.is_none());
}

fn tool_permission_responder(
    decision: ToolPermissionDecision,
) -> (
    Arc<HumanInteractionCoordinator>,
    AgentEventSink,
    Arc<std::sync::Mutex<Vec<AgentEvent>>>,
) {
    let coordinator = Arc::new(HumanInteractionCoordinator::new());
    let (agent_sink, agent_events) = capture_agent_events();
    let sink_coordinator = coordinator.clone();
    let sink: AgentEventSink = Arc::new(move |event| {
        if let AgentEvent::HumanInteractionRequested { request } = &event {
            let coordinator = sink_coordinator.clone();
            let request_id = request.request_id;
            tokio::spawn(async move {
                let _ = coordinator
                    .submit_tool_permission_response(request_id, decision)
                    .await;
            });
        }
        agent_sink(event);
    });
    (coordinator, sink, agent_events)
}

#[tokio::test]
async fn steering_skip_emits_tool_and_message_lifecycle_for_skipped_calls() {
    let (runner, _requests) = test_runner(ProviderScenario::MutatingBatchThenComplete);
    let (agent_sink, agent_events) = capture_agent_events();
    let active_calls = Arc::new(AtomicUsize::new(0));
    let max_active_calls = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("run tools")]);
    request.tools = vec![
        tracked_success_tool(
            "apply_patch",
            Duration::from_millis(40),
            active_calls.clone(),
            max_active_calls.clone(),
        ),
        tracked_success_tool(
            "read",
            Duration::from_millis(40),
            active_calls,
            max_active_calls,
        ),
    ];
    request.approval_policy = ApprovalPolicy::always();
    request.agent_event_sink = Some(agent_sink);

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
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert!(
        tool_result_ids_from_messages(&result.messages)
            .iter()
            .any(|id| id == "safe-read-2"),
        "expected skipped tool result for safe-read-2"
    );

    let events = agent_events.lock().expect("agent event lock");
    let skipped_tool_start = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ToolExecutionStart { tool_call_id, .. } if tool_call_id == "safe-read-2"
        )
    });
    let skipped_tool_end = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ToolExecutionEnd { tool_call_id, .. } if tool_call_id == "safe-read-2"
        )
    });
    let skipped_msg_start = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::MessageStart { message }
                if message.role == crate::types::Role::Tool
                    && tool_result_id_from_message(message) == Some("safe-read-2")
        )
    });
    let skipped_msg_end = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::MessageEnd { message }
                if message.role == crate::types::Role::Tool
                    && tool_result_id_from_message(message) == Some("safe-read-2")
        )
    });

    assert!(
        skipped_tool_start,
        "expected ToolExecutionStart for skipped call"
    );
    assert!(
        skipped_tool_end,
        "expected ToolExecutionEnd for skipped call"
    );
    assert!(
        skipped_msg_start,
        "expected MessageStart for skipped tool result"
    );
    assert!(
        skipped_msg_end,
        "expected MessageEnd for skipped tool result"
    );
}

#[tokio::test]
async fn tool_failures_are_bounded_with_deterministic_reason() {
    let (runner, _requests) = test_runner(ProviderScenario::RepeatedToolFailure);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("run tool")]);
    request.tools = vec![failing_tool()];
    request.approval_policy = ApprovalPolicy::always();
    request.event_sink = Some(sink);
    request
        .metadata
        .insert("runner.max_iterations".to_string(), "20".to_string());
    request
        .metadata
        .insert("runner.max_tool_failures".to_string(), "2".to_string());

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    let expected_error = "tool call failure limit reached (max_failures=2, consecutive_failures=2)";
    assert_eq!(result.status, RunStatus::Failed);
    assert_eq!(result.error.as_deref(), Some(expected_error));
    assert!(
        !result.messages.is_empty(),
        "failed runs should still expose conversation state"
    );
    let result_tool_ids = tool_result_ids_from_messages(&result.messages);
    assert_eq!(result_tool_ids.len(), 2);
    assert!(result_tool_ids.iter().all(|id| id == "tool-call-1"));

    let events = events.lock().expect("event lock");
    let failure_events: Vec<String> = events
        .iter()
        .filter_map(|event| match &event.payload {
            RunEventPayload::Lifecycle {
                state: RunLifecycle::Failed { error },
            } => Some(error.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        failure_events.last().map(String::as_str),
        Some(expected_error)
    );

    let tool_results = events
        .iter()
        .filter(|event| matches!(event.payload, RunEventPayload::ToolResult { .. }))
        .count();
    assert_eq!(tool_results, 2);
}

#[tokio::test]

async fn parallel_safe_tools_execute_concurrently_and_append_results_in_call_order() {
    let (runner, requests) = test_runner(ProviderScenario::ParallelSafeBatchThenComplete);
    let (sink, events) = capture_events();
    let active_calls = Arc::new(AtomicUsize::new(0));
    let max_active_calls = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("parallel tools")]);
    request.tools = vec![
        tracked_safe_success_tool(
            "read",
            Duration::from_millis(150),
            active_calls.clone(),
            max_active_calls.clone(),
        ),
        tracked_safe_success_tool(
            "ls",
            Duration::from_millis(150),
            active_calls,
            max_active_calls.clone(),
        ),
    ];
    request.approval_policy = ApprovalPolicy::always();
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert!(max_active_calls.load(Ordering::SeqCst) >= 2);

    let requests = requests.lock().expect("request lock");
    assert!(requests.len() >= 2);
    let second_request_messages = &requests[1].messages;
    assert_eq!(
        assistant_tool_call_message_count(second_request_messages),
        1
    );
    assert_eq!(
        tool_result_ids_from_messages(second_request_messages),
        vec!["safe-read-1".to_string(), "safe-ls-2".to_string()]
    );

    let events = events.lock().expect("event lock");
    assert_eq!(
        tool_result_ids_from_events(events.as_slice()),
        vec!["safe-read-1".to_string(), "safe-ls-2".to_string()]
    );
    assert_eq!(
        tool_call_completed_ids_from_events(events.as_slice()),
        vec!["safe-read-1".to_string(), "safe-ls-2".to_string()]
    );
}

#[tokio::test]
async fn mutating_tools_remain_serialized_even_when_safe_tools_exist() {
    let (runner, requests) = test_runner(ProviderScenario::MutatingBatchThenComplete);
    let (sink, events) = capture_events();
    let active_calls = Arc::new(AtomicUsize::new(0));
    let max_active_calls = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("mutating tools")]);
    request.tools = vec![
        tracked_success_tool(
            "apply_patch",
            Duration::from_millis(150),
            active_calls.clone(),
            max_active_calls.clone(),
        ),
        tracked_safe_success_tool(
            "read",
            Duration::from_millis(150),
            active_calls.clone(),
            max_active_calls.clone(),
        ),
        tracked_safe_success_tool(
            "ls",
            Duration::from_millis(150),
            active_calls,
            max_active_calls.clone(),
        ),
    ];
    request.approval_policy = ApprovalPolicy::always();
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(max_active_calls.load(Ordering::SeqCst), 2);

    let requests = requests.lock().expect("request lock");
    assert!(requests.len() >= 2);
    let second_request_messages = &requests[1].messages;
    assert_eq!(
        assistant_tool_call_message_count(second_request_messages),
        1
    );
    assert_eq!(
        tool_result_ids_from_messages(second_request_messages),
        vec![
            "mutating-call-1".to_string(),
            "safe-read-2".to_string(),
            "safe-ls-3".to_string(),
        ]
    );

    let events = events.lock().expect("event lock");
    assert_eq!(
        tool_result_ids_from_events(events.as_slice()),
        vec![
            "mutating-call-1".to_string(),
            "safe-read-2".to_string(),
            "safe-ls-3".to_string(),
        ]
    );
    assert_eq!(
        tool_call_completed_ids_from_events(events.as_slice()),
        vec![
            "mutating-call-1".to_string(),
            "safe-read-2".to_string(),
            "safe-ls-3".to_string(),
        ]
    );
}

#[tokio::test]
async fn mixed_text_and_parallel_tools_are_batched_before_single_followup() {
    let (runner, requests) = test_runner(ProviderScenario::MixedTextAndParallelBatchThenComplete);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("mixed stream")]);
    request.tools = vec![
        tracked_safe_success_tool(
            "read",
            Duration::from_millis(80),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        ),
        tracked_safe_success_tool(
            "ls",
            Duration::from_millis(80),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        ),
    ];
    request.approval_policy = ApprovalPolicy::always();
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    let second_request_messages = &requests[1].messages;
    assert_eq!(
        assistant_tool_call_message_count(second_request_messages),
        1
    );
    assert_eq!(
        tool_result_ids_from_messages(second_request_messages),
        vec!["mixed-read-1".to_string(), "mixed-ls-2".to_string()]
    );
    assert!(assistant_text_content(second_request_messages).contains("Gathering context."));

    let events = events.lock().expect("event lock");
    assert_eq!(
        tool_result_ids_from_events(events.as_slice()),
        vec!["mixed-read-1".to_string(), "mixed-ls-2".to_string()]
    );
}

#[tokio::test]
async fn duplicate_tool_call_deltas_are_deduplicated_by_call_id() {
    let (runner, requests) = test_runner(ProviderScenario::DuplicateToolCallDeltaThenComplete);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("dup tool call")]);
    request.tools = vec![tracked_success_tool(
        "read",
        Duration::from_millis(50),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    )];
    request.approval_policy = ApprovalPolicy::always();
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    let second_request_messages = &requests[1].messages;
    assert_eq!(
        tool_result_ids_from_messages(second_request_messages),
        vec!["dup-read-1".to_string()]
    );
    let calls = assistant_tool_calls(second_request_messages);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "dup-read-1");
    assert_eq!(calls[0].arguments["path"], serde_json::json!("second"));

    let events = events.lock().expect("event lock");
    let tool_starts = events
        .iter()
        .filter(|event| matches!(event.payload, RunEventPayload::ToolCallStarted { .. }))
        .count();
    assert_eq!(tool_starts, 1);
    assert_eq!(
        tool_result_ids_from_events(events.as_slice()),
        vec!["dup-read-1".to_string()]
    );
}

#[tokio::test]
async fn approval_uses_same_duplicate_named_tool_instance_as_execution() {
    let (runner, _requests) = test_runner(ProviderScenario::DuplicateToolCallDeltaThenComplete);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("dup tool names")]);
    let unsafe_read: Arc<dyn Tool> = Arc::new(AgentTool::new(
        "read",
        "custom read with side effects",
        AgentToolParameters::empty(),
        |_args, _ctx: ToolExecutionContext| async move {
            Ok(serde_json::json!({ "executed": "unsafe-read" }))
        },
    ));
    let safe_read: Arc<dyn Tool> = Arc::new(
        AgentTool::new(
            "read",
            "safe read",
            AgentToolParameters::empty(),
            |_args, _ctx: ToolExecutionContext| async move {
                Ok(serde_json::json!({ "executed": "safe-read" }))
            },
        )
        .with_static_safety(
            ToolSafetyPlan::safe_read_only(ToolSafetyKind::Read),
            read_only_safety_summary(ToolSafetyKind::Read),
        ),
    );
    request.tools = vec![unsafe_read, safe_read];
    request.approval_policy = ApprovalPolicy::ask();
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Completed);
    let events = events.lock().expect("event lock");
    let approval_requests = events
        .iter()
        .filter(|event| matches!(event.payload, RunEventPayload::ApprovalRequired { .. }))
        .count();
    assert_eq!(approval_requests, 1);

    let tool_results = events
        .iter()
        .filter_map(|event| match &event.payload {
            RunEventPayload::ToolResult { result } => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_results.len(), 1);
    assert_eq!(tool_results[0].result["error"], "approval declined");
    assert!(tool_results[0].result.get("executed").is_none());
}

#[tokio::test]
async fn tool_permission_denial_returns_tool_denial_result() {
    let (runner, _requests) = test_runner(ProviderScenario::DuplicateToolCallDeltaThenComplete);
    let (coordinator, agent_sink, _agent_events) =
        tool_permission_responder(ToolPermissionDecision::Deny);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("deny tool")]);
    request.tools = vec![Arc::new(AgentTool::new(
        "read",
        "unsafe read",
        AgentToolParameters::empty(),
        |_args, _ctx: ToolExecutionContext| async move { Ok(serde_json::json!({ "executed": true })) },
    ))];
    request.approval_policy = ApprovalPolicy::ask();
    request.event_sink = Some(sink);
    request.agent_event_sink = Some(agent_sink);
    request.human_interaction_coordinator = Some(coordinator);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Completed);
    let events = events.lock().expect("event lock");
    let tool_results = events
        .iter()
        .filter_map(|event| match &event.payload {
            RunEventPayload::ToolResult { result } => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_results.len(), 1);
    assert_eq!(tool_results[0].result["error"], "approval declined");
    assert!(tool_results[0].is_error);
}

#[tokio::test]
async fn tool_permission_cancel_aborts_run() {
    let (runner, _requests) = test_runner(ProviderScenario::DuplicateToolCallDeltaThenComplete);
    let (coordinator, agent_sink, _agent_events) =
        tool_permission_responder(ToolPermissionDecision::Cancel);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("cancel tool")]);
    request.tools = vec![Arc::new(AgentTool::new(
        "read",
        "unsafe read",
        AgentToolParameters::empty(),
        |_args, _ctx: ToolExecutionContext| async move { Ok(serde_json::json!({ "executed": true })) },
    ))];
    request.approval_policy = ApprovalPolicy::ask();
    request.event_sink = Some(sink);
    request.agent_event_sink = Some(agent_sink);
    request.human_interaction_coordinator = Some(coordinator);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Canceled);
    let events = events.lock().expect("event lock");
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.payload, RunEventPayload::ToolResult { .. })),
        "canceled permission should abort before tool result"
    );
}

#[tokio::test]
async fn stream_end_without_done_falls_back_to_tool_execution_and_completion() {
    let (runner, requests) = test_runner(ProviderScenario::StreamEndsWithoutDoneThenComplete);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(
        test_model(),
        vec![ModelMessage::user("fallback completion")],
    );
    request.tools = vec![tracked_success_tool(
        "read",
        Duration::from_millis(50),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    )];
    request.approval_policy = ApprovalPolicy::always();
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    let second_request_messages = &requests[1].messages;
    assert_eq!(
        tool_result_ids_from_messages(second_request_messages),
        vec!["fallback-read-1".to_string()]
    );

    let events = events.lock().expect("event lock");
    assert_eq!(
        tool_result_ids_from_events(events.as_slice()),
        vec!["fallback-read-1".to_string()]
    );
    assert!(
        events.iter().all(|event| {
            !matches!(
                event.payload,
                RunEventPayload::Lifecycle {
                    state: RunLifecycle::Failed { .. },
                }
            )
        }),
        "stream-end fallback should not emit failed lifecycle"
    );
}
