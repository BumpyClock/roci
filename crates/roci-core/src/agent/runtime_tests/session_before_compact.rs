use super::support::*;
use super::*;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn session_before_compact_can_cancel_compaction() {
    let mut config = test_agent_config();
    config.compaction.keep_recent_tokens = 1;
    config.session_before_compact = Some(Arc::new(|_payload| {
        Box::pin(async { Ok(SessionSummaryHookOutcome::Cancel) })
    }));
    let agent = AgentRuntime::new(test_registry(), test_config(), config);
    let original_messages = vec![
        ModelMessage::user("first"),
        ModelMessage::assistant("second"),
        ModelMessage::user("third"),
    ];
    agent
        .replace_messages(original_messages.clone())
        .await
        .expect("messages should be set");

    let error = agent
        .compact()
        .await
        .expect_err("compaction cancel should return an explicit error");

    assert!(error.to_string().contains("session_before_compact"));
    assert_eq!(agent.messages().await, original_messages);
}

#[tokio::test]
async fn session_before_compact_accepts_full_override_contract() {
    let mut config = test_agent_config();
    config.compaction.keep_recent_tokens = 1;
    config.session_before_compact = Some(Arc::new(|payload| {
        Box::pin(async move {
            let mut all_messages = payload.to_summarize.messages.clone();
            all_messages.extend(payload.turn_prefix.messages.clone());
            all_messages.extend(payload.kept.messages.clone());
            let first_kept_entry_id = all_messages.len() - 1;
            let tokens_before = all_messages[..first_kept_entry_id]
                .iter()
                .map(estimate_message_tokens)
                .sum::<usize>();
            Ok(SessionSummaryHookOutcome::OverrideCompaction(
                SessionCompactionOverride {
                    summary: "hooked full override summary".to_string(),
                    first_kept_entry_id,
                    tokens_before,
                    details: Some("keep trailing context verbatim".to_string()),
                },
            ))
        })
    }));
    let agent = AgentRuntime::new(test_registry(), test_config(), config);
    agent
        .replace_messages(vec![
            ModelMessage::user("first"),
            ModelMessage::assistant("second"),
            ModelMessage::user("latest request"),
            ModelMessage::assistant("latest answer"),
        ])
        .await
        .expect("messages should be set");

    agent
        .compact()
        .await
        .expect("compaction with full override should succeed");

    let compacted = agent.messages().await;
    assert!(
        compacted
            .iter()
            .any(|message| message.text().contains("hooked full override summary")),
        "compaction summary should use full override summary"
    );
    assert!(
        compacted
            .iter()
            .any(|message| message.text().contains("Compaction override details")),
        "compaction summary should include override details"
    );
    assert!(
        compacted
            .iter()
            .any(|message| message.text().contains("latest answer")),
        "latest messages should remain after override compaction"
    );
}

#[tokio::test]
async fn session_before_compact_rejects_invalid_override() {
    let mut config = test_agent_config();
    config.compaction.keep_recent_tokens = 1;
    config.session_before_compact = Some(Arc::new(|_payload| {
        Box::pin(async {
            Ok(SessionSummaryHookOutcome::OverrideCompaction(
                SessionCompactionOverride {
                    summary: "invalid boundary".to_string(),
                    first_kept_entry_id: 0,
                    tokens_before: 0,
                    details: None,
                },
            ))
        })
    }));
    let agent = AgentRuntime::new(test_registry(), test_config(), config);
    agent
        .replace_messages(vec![
            ModelMessage::user("first"),
            ModelMessage::assistant("second"),
            ModelMessage::user("third"),
        ])
        .await
        .expect("messages should be set");

    let error = agent
        .compact()
        .await
        .expect_err("invalid override should fail compaction");
    assert!(error.to_string().contains("first_kept_entry_id"));
}

#[tokio::test]
async fn session_before_compact_payload_exposes_cancellation_token() {
    let is_canceled = Arc::new(Mutex::new(None));
    let is_canceled_for_hook = is_canceled.clone();
    let mut config = test_agent_config();
    config.compaction.keep_recent_tokens = 1;
    config.session_before_compact = Some(Arc::new(move |payload| {
        let is_canceled_for_hook = is_canceled_for_hook.clone();
        Box::pin(async move {
            *is_canceled_for_hook
                .lock()
                .expect("capture lock should not fail") =
                Some(payload.cancellation_token.is_cancelled());
            Ok(SessionSummaryHookOutcome::OverrideSummary(
                "cancellation token capture".to_string(),
            ))
        })
    }));
    let agent = AgentRuntime::new(test_registry(), test_config(), config);
    agent
        .replace_messages(vec![
            ModelMessage::user("first"),
            ModelMessage::assistant("second"),
            ModelMessage::user("third"),
        ])
        .await
        .expect("messages should be set");

    agent
        .compact()
        .await
        .expect("compaction should succeed with summary override");

    assert_eq!(
        *is_canceled.lock().expect("capture lock should not fail"),
        Some(false)
    );
}

#[tokio::test]
async fn session_before_compact_can_override_summary_and_receive_preparation_payload() {
    let payload_capture = Arc::new(Mutex::new(None));
    let payload_capture_for_hook = payload_capture.clone();
    let mut config = test_agent_config();
    config.compaction.keep_recent_tokens = 1;
    config.session_before_compact = Some(Arc::new(move |payload| {
        let payload_capture_for_hook = payload_capture_for_hook.clone();
        Box::pin(async move {
            *payload_capture_for_hook
                .lock()
                .expect("payload capture should lock") = Some(payload);
            Ok(SessionSummaryHookOutcome::OverrideSummary(
                "hooked compaction summary".to_string(),
            ))
        })
    }));
    let agent = AgentRuntime::new(test_registry(), test_config(), config);
    agent
        .replace_messages(vec![
            ModelMessage::user("first"),
            assistant_tool_call(
                "call_1",
                "read_file",
                serde_json::json!({"path": "src/a.rs"}),
            ),
            ModelMessage::tool_result("call_1", serde_json::json!({"ok": true}), false),
            ModelMessage::assistant("latest"),
        ])
        .await
        .expect("messages should be set");

    agent
        .compact()
        .await
        .expect("compaction with override should succeed");

    let captured = payload_capture
        .lock()
        .expect("payload capture should lock")
        .clone()
        .expect("hook payload should be captured");
    assert_eq!(captured.settings.keep_recent_tokens, 1);
    assert!(captured.to_summarize.token_count > 0);
    assert!(captured
        .to_summarize
        .file_operations
        .read_files
        .contains("src/a.rs"));
    assert!(!captured.cancellation_token.is_cancelled());

    let compacted = agent.messages().await;
    assert!(
        compacted
            .iter()
            .any(|message| message.text().contains("hooked compaction summary")),
        "compaction summary should use hook override text"
    );
}
