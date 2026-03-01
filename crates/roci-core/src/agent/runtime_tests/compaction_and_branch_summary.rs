use super::support::*;
use super::*;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn compact_replaces_history_with_summary_and_preserves_system_prompt() {
    let created_models = Arc::new(Mutex::new(Vec::new()));
    let registry = registry_with_summary_provider("stub", "summarized context", created_models);
    let mut config = test_agent_config();
    config.model = "stub:run-model".parse().expect("stub model should parse");
    config.compaction.keep_recent_tokens = 1;
    let agent = AgentRuntime::new(registry, test_config(), config);

    agent
        .replace_messages(vec![
            ModelMessage::system("You are precise"),
            ModelMessage::user("first"),
            ModelMessage::assistant("answer"),
            ModelMessage::user("latest"),
        ])
        .await
        .expect("messages should be set");

    agent
        .compact()
        .await
        .expect("manual compaction should succeed");
    let messages = agent.messages().await;

    assert_eq!(messages[0].role, Role::System);
    assert_eq!(messages[0].text(), "You are precise");
    assert!(
        messages
            .iter()
            .any(|message| message.text().contains("<compaction_summary>")),
        "compacted history should include a summary wrapper"
    );
    assert!(
        messages.len() < 4,
        "manual compaction should replace part of the history"
    );
}

#[tokio::test]
async fn compact_uses_configured_compaction_model_when_present() {
    let created_models = Arc::new(Mutex::new(Vec::new()));
    let registry = registry_with_summary_provider("summary", "summary", created_models.clone());
    let mut config = test_agent_config();
    config.model = "run:model".parse().expect("run model should parse");
    config.compaction.model = Some("summary:compact-model".to_string());
    config.compaction.keep_recent_tokens = 1;
    let agent = AgentRuntime::new(registry, test_config(), config);

    agent
        .replace_messages(vec![
            ModelMessage::user("first"),
            ModelMessage::assistant("second"),
            ModelMessage::user("third"),
        ])
        .await
        .expect("messages should be set");

    agent.compact().await.expect("compaction should succeed");

    let created_models = created_models.lock().expect("created models lock");
    assert_eq!(created_models.as_slice(), ["compact-model"]);
}

#[tokio::test]
async fn summarize_branch_entries_uses_branch_summary_model_override_when_present() {
    let created_models = Arc::new(Mutex::new(Vec::new()));
    let registry =
        registry_with_summary_provider("summary", "branch summary", created_models.clone());
    let mut config = test_agent_config();
    config.model = "run:run-model".parse().expect("run model should parse");
    let agent = AgentRuntime::new(registry, test_config(), config);

    let settings = BranchSummarySettings {
        reserve_tokens: 10_000,
        model: Some("summary:branch-model".to_string()),
    };

    agent
        .summarize_branch_entries(
            vec![
                ModelMessage::user("branch a"),
                ModelMessage::assistant("branch b"),
            ],
            &settings,
        )
        .await
        .expect("branch summary should succeed");

    let created_models = created_models.lock().expect("created models lock");
    assert_eq!(created_models.as_slice(), ["branch-model"]);
}

#[tokio::test]
async fn summarize_branch_entries_returns_branch_summary_message_with_cumulative_file_tracking() {
    let created_models = Arc::new(Mutex::new(Vec::new()));
    let registry = registry_with_summary_provider("stub", "new progress", created_models);
    let mut config = test_agent_config();
    config.model = "stub:run-model".parse().expect("run model should parse");
    let agent = AgentRuntime::new(registry, test_config(), config);

    let prior_summary = PiMonoSummary {
        read_files: BTreeSet::from(["src/previous_read.rs".to_string()]),
        modified_files: BTreeSet::from(["src/previous_mod.rs".to_string()]),
        ..PiMonoSummary::default()
    };
    agent
        .replace_messages(vec![ModelMessage::user(format!(
            "<branch_summary>\n{}\n</branch_summary>",
            serialize_pi_mono_summary(&prior_summary)
        ))])
        .await
        .expect("history should be set");

    let settings = BranchSummarySettings {
        reserve_tokens: 10_000,
        model: None,
    };
    let summary_message = agent
        .summarize_branch_entries(
            vec![
                assistant_tool_call(
                    "call_1",
                    "read_file",
                    serde_json::json!({"path": "src/new_read.rs"}),
                ),
                assistant_tool_call(
                    "call_2",
                    "write_file",
                    serde_json::json!({"path": "src/new_mod.rs"}),
                ),
                ModelMessage::assistant("done"),
            ],
            &settings,
        )
        .await
        .expect("branch summary should succeed");

    assert_eq!(summary_message.kind(), "branch_summary");

    let llm = summary_message
        .to_llm_message()
        .expect("branch summary should convert to llm message");
    let text = llm.text();
    assert!(text.contains("<branch_summary>"));
    assert!(text.contains("src/previous_read.rs"));
    assert!(text.contains("src/new_read.rs"));
    assert!(text.contains("src/previous_mod.rs"));
    assert!(text.contains("src/new_mod.rs"));
    assert!(text.contains("</branch_summary>"));
}
