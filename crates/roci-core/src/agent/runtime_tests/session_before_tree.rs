use super::support::*;
use super::*;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn session_before_tree_can_override_branch_summary_and_receive_preparation_payload() {
    let payload_capture = Arc::new(Mutex::new(None));
    let payload_capture_for_hook = payload_capture.clone();
    let mut config = test_agent_config();
    config.model = "run:run-model".parse().expect("run model should parse");
    config.session_before_tree = Some(Arc::new(move |payload| {
        let payload_capture_for_hook = payload_capture_for_hook.clone();
        Box::pin(async move {
            *payload_capture_for_hook
                .lock()
                .expect("payload capture should lock") = Some(payload);
            Ok(SessionSummaryHookOutcome::OverrideSummary(
                "hooked branch summary".to_string(),
            ))
        })
    }));
    let agent = AgentRuntime::new(test_registry(), test_config(), config);
    let settings = BranchSummarySettings {
        reserve_tokens: 10_000,
        model: None,
    };

    let summary = agent
        .summarize_branch_entries(
            vec![
                assistant_tool_call(
                    "call_1",
                    "read_file",
                    serde_json::json!({"path": "src/t.rs"}),
                ),
                ModelMessage::assistant("done"),
            ],
            &settings,
        )
        .await
        .expect("branch summary with hook override should succeed");

    let captured = payload_capture
        .lock()
        .expect("payload capture should lock")
        .clone()
        .expect("hook payload should be captured");
    assert_eq!(captured.settings.reserve_tokens, 10_000);
    assert!(captured.to_summarize.token_count > 0);
    assert!(captured
        .to_summarize
        .file_operations
        .read_files
        .contains("src/t.rs"));

    let summary_text = summary.text().expect("branch summary should have text");
    assert!(summary_text.contains("hooked branch summary"));
}

#[tokio::test]
async fn session_before_tree_can_cancel_branch_summary() {
    let mut config = test_agent_config();
    config.model = "run:run-model".parse().expect("run model should parse");
    config.session_before_tree = Some(Arc::new(|_payload| {
        Box::pin(async { Ok(SessionSummaryHookOutcome::Cancel) })
    }));
    let agent = AgentRuntime::new(test_registry(), test_config(), config);
    let settings = BranchSummarySettings {
        reserve_tokens: 10_000,
        model: None,
    };

    let error = agent
        .summarize_branch_entries(vec![ModelMessage::user("entry")], &settings)
        .await
        .expect_err("branch summary should fail when hook cancels");
    assert!(error.to_string().contains("session_before_tree"));
}
