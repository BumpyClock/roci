use super::*;

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
    request.approval_policy = ApprovalPolicy::Always;
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
    request.approval_policy = ApprovalPolicy::Always;
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
        tracked_success_tool(
            "read",
            Duration::from_millis(150),
            active_calls.clone(),
            max_active_calls.clone(),
        ),
        tracked_success_tool(
            "ls",
            Duration::from_millis(150),
            active_calls,
            max_active_calls.clone(),
        ),
    ];
    request.approval_policy = ApprovalPolicy::Always;
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
        tracked_success_tool(
            "read",
            Duration::from_millis(150),
            active_calls,
            max_active_calls.clone(),
        ),
    ];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(max_active_calls.load(Ordering::SeqCst), 1);

    let requests = requests.lock().expect("request lock");
    assert!(requests.len() >= 2);
    let second_request_messages = &requests[1].messages;
    assert_eq!(
        assistant_tool_call_message_count(second_request_messages),
        1
    );
    assert_eq!(
        tool_result_ids_from_messages(second_request_messages),
        vec!["mutating-call-1".to_string(), "safe-read-2".to_string()]
    );

    let events = events.lock().expect("event lock");
    assert_eq!(
        tool_result_ids_from_events(events.as_slice()),
        vec!["mutating-call-1".to_string(), "safe-read-2".to_string()]
    );
    assert_eq!(
        tool_call_completed_ids_from_events(events.as_slice()),
        vec!["mutating-call-1".to_string(), "safe-read-2".to_string()]
    );
}

#[tokio::test]
async fn mixed_text_and_parallel_tools_are_batched_before_single_followup() {
    let (runner, requests) = test_runner(ProviderScenario::MixedTextAndParallelBatchThenComplete);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("mixed stream")]);
    request.tools = vec![
        tracked_success_tool(
            "read",
            Duration::from_millis(80),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        ),
        tracked_success_tool(
            "ls",
            Duration::from_millis(80),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        ),
    ];
    request.approval_policy = ApprovalPolicy::Always;
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
    request.approval_policy = ApprovalPolicy::Always;
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
    request.approval_policy = ApprovalPolicy::Always;
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
