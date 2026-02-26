use super::*;
async fn agent_message_lifecycle_events_emit_for_text_stream() {
    let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let (agent_sink, agent_events) = capture_agent_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.agent_event_sink = Some(agent_sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let events = agent_events.lock().expect("agent event lock");
    let start_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageStart { message }
                    if message.role == crate::types::Role::Assistant
            )
        })
        .expect("expected assistant MessageStart");
    let update_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageUpdate {
                    message,
                    assistant_message_event,
                    ..
                } if message.role == crate::types::Role::Assistant
                    && assistant_message_event.event_type == StreamEventType::TextDelta
                    && assistant_message_event.text == "done"
            )
        })
        .expect("expected MessageUpdate(done)");
    let end_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageEnd { message }
                    if message.role == crate::types::Role::Assistant
            )
        })
        .expect("expected assistant MessageEnd");
    assert!(start_idx < update_idx);
    assert!(update_idx < end_idx);
}

#[tokio::test]
async fn message_lifecycle_events_cover_prompt_and_tool_results() {
    let (runner, _requests) = test_runner(ProviderScenario::ToolUpdateThenComplete);
    let (agent_sink, agent_events) = capture_agent_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("run update tool")]);
    request.tools = vec![update_streaming_tool(false)];
    request.approval_policy = ApprovalPolicy::Always;
    request.agent_event_sink = Some(agent_sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let events = agent_events.lock().expect("agent event lock");
    let user_start_count = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::MessageStart { message }
                    if message.role == crate::types::Role::User
            )
        })
        .count();
    let user_end_count = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::MessageEnd { message }
                    if message.role == crate::types::Role::User
            )
        })
        .count();
    assert_eq!(user_start_count, 1);
    assert_eq!(user_end_count, 1);

    let tool_start = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::MessageStart { message }
                if message.role == crate::types::Role::Tool
                    && tool_result_id_from_message(message) == Some("update-tool-1")
        )
    });
    let tool_end = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::MessageEnd { message }
                if message.role == crate::types::Role::Tool
                    && tool_result_id_from_message(message) == Some("update-tool-1")
        )
    });
    assert!(
        tool_start,
        "expected tool result MessageStart for update-tool-1"
    );
    assert!(
        tool_end,
        "expected tool result MessageEnd for update-tool-1"
    );
}

#[tokio::test]
async fn agent_message_end_is_emitted_before_failure_terminal_event() {
    let (runner, _requests) = test_runner(ProviderScenario::TextThenStreamError);
    let (agent_sink, agent_events) = capture_agent_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.agent_event_sink = Some(agent_sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Failed);
    assert!(result
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("upstream stream failure"));

    let events = agent_events.lock().expect("agent event lock");
    let start_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageStart { message }
                    if message.role == crate::types::Role::Assistant
            )
        })
        .expect("expected assistant MessageStart");
    let update_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageUpdate {
                    message,
                    assistant_message_event,
                    ..
                } if message.role == crate::types::Role::Assistant
                    && assistant_message_event.event_type == StreamEventType::TextDelta
                    && assistant_message_event.text == "partial"
            )
        })
        .expect("expected MessageUpdate(partial)");
    let message_end_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageEnd { message }
                    if message.role == crate::types::Role::Assistant
            )
        })
        .expect("expected assistant MessageEnd");
    let agent_end_idx = events
        .iter()
        .position(|event| matches!(event, AgentEvent::AgentEnd { .. }))
        .expect("expected AgentEnd");
    assert!(start_idx < update_idx);
    assert!(update_idx < message_end_idx);
    assert!(message_end_idx < agent_end_idx);
}

#[tokio::test]
async fn tool_execution_updates_stream_with_deterministic_order() {
    let (runner, _requests) = test_runner(ProviderScenario::ToolUpdateThenComplete);
    let (agent_sink, agent_events) = capture_agent_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("run update tool")]);
    request.tools = vec![update_streaming_tool(false)];
    request.approval_policy = ApprovalPolicy::Always;
    request.agent_event_sink = Some(agent_sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(
        tool_result_ids_from_messages(&result.messages),
        vec!["update-tool-1".to_string()]
    );

    let events = agent_events.lock().expect("agent event lock");
    let mut sequence: Vec<String> = Vec::new();
    for event in events.iter() {
        match event {
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                ..
            } if tool_call_id == "update-tool-1" && tool_name == "update_tool" => {
                sequence.push("start".to_string());
            }
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                partial_result,
                ..
            } if tool_call_id == "update-tool-1" => {
                let step = partial_result
                    .details
                    .get("step")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or_default();
                sequence.push(format!("update-{step}"));
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                is_error,
                ..
            } if tool_call_id == "update-tool-1" => {
                assert!(!is_error);
                sequence.push("end".to_string());
            }
            _ => {}
        }
    }
    assert_eq!(
        sequence,
        vec![
            "start".to_string(),
            "update-1".to_string(),
            "update-2".to_string(),
            "end".to_string(),
        ]
    );
}

#[tokio::test]
async fn canceling_during_tool_execution_emits_error_end_event() {
    let (runner, _requests) = test_runner(ProviderScenario::ToolUpdateThenComplete);
    let (agent_sink, agent_events) = capture_agent_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("cancel update tool")]);
    request.tools = vec![update_streaming_tool(true)];
    request.approval_policy = ApprovalPolicy::Always;
    request.agent_event_sink = Some(agent_sink);

    let mut handle = runner.start(request).await.expect("start run");
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(handle.abort(), "abort should be accepted");

    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Canceled);

    let events = agent_events.lock().expect("agent event lock");
    let end_event = events.iter().find_map(|event| match event {
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            is_error,
            ..
        } if tool_call_id == "update-tool-1" => Some(*is_error),
        _ => None,
    });
    assert_eq!(end_event, Some(true));
}
