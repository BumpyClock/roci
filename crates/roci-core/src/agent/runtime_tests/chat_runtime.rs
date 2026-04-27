use super::chat::{MessageStatus, TurnStatus};
use super::support::*;
use super::*;
use crate::agent_loop::RunStatus;
use crate::types::{ModelMessage, Role};

fn runtime_with_chat_provider() -> AgentRuntime {
    let registry = registry_with_streaming_provider("stub", 8, 3);
    let mut config = test_agent_config();
    config.model = "stub:chat-runtime"
        .parse()
        .expect("stub model should parse");
    AgentRuntime::new(registry, test_config(), config)
}

#[tokio::test]
async fn read_snapshot_initially_returns_default_thread() {
    let agent = runtime_with_chat_provider();

    let snapshot = agent.read_snapshot().await;

    assert_eq!(snapshot.schema_version, 1);
    assert_eq!(snapshot.threads.len(), 1);

    let default_thread = snapshot.threads[0].clone();
    let read_thread = agent
        .read_thread(default_thread.thread_id)
        .await
        .expect("default thread should be readable");

    assert_eq!(read_thread, default_thread);
    assert_eq!(default_thread.revision, 0);
    assert_eq!(default_thread.last_seq, 0);
    assert!(default_thread.active_turn_id.is_none());
    assert!(default_thread.turns.is_empty());
    assert!(default_thread.messages.is_empty());
    assert!(default_thread.tools.is_empty());
}

#[tokio::test]
async fn prompt_projects_completed_turn_and_host_ready_messages() {
    let agent = runtime_with_chat_provider();
    let before = agent.read_snapshot().await.threads[0].clone();

    let result = agent.prompt("hello").await.expect("prompt should run");

    assert_eq!(
        result.status,
        RunStatus::Completed,
        "error: {:?}",
        result.error
    );

    let snapshot = agent.read_snapshot().await;
    let thread = snapshot.threads[0].clone();
    let read_thread = agent
        .read_thread(thread.thread_id)
        .await
        .expect("thread should be readable");

    assert_eq!(read_thread, thread);
    assert!(
        thread.last_seq > before.last_seq,
        "prompt should advance chat sequence"
    );
    assert_eq!(thread.turns.len(), 1);
    assert_eq!(thread.turns[0].status, TurnStatus::Completed);
    assert!(thread.turns[0].completed_at.is_some());
    assert!(thread.active_turn_id.is_none());
    assert_eq!(thread.messages.len(), 2);
    assert_eq!(thread.messages[0].payload.role, Role::User);
    assert_eq!(thread.messages[0].payload.text(), "hello");
    assert_eq!(thread.messages[1].payload.role, Role::Assistant);
    assert_eq!(thread.messages[1].payload.text(), "hello");
    assert_eq!(thread.turns[0].turn_id.revision(), thread.revision);
    assert!(
        thread.messages[0].message_id.ordinal() < thread.messages[1].message_id.ordinal(),
        "message ids should preserve prompt/response order"
    );
    assert!(thread
        .messages
        .iter()
        .all(|message| message.status == MessageStatus::Completed));
    assert_eq!(
        thread.turns[0].message_ids,
        thread
            .messages
            .iter()
            .map(|message| message.message_id)
            .collect::<Vec<_>>()
    );
    assert!(thread
        .messages
        .iter()
        .all(|message| message.message_id.revision() == thread.revision));

    let payload_text = thread
        .messages
        .iter()
        .map(|message| message.payload.text())
        .collect::<Vec<_>>();
    assert_eq!(payload_text, ["hello".to_string(), "hello".to_string()]);
}

#[tokio::test]
async fn replace_messages_imports_exact_history_without_active_turn() {
    let agent = runtime_with_chat_provider();
    let initial = agent.read_snapshot().await.threads[0].clone();
    let history = vec![
        ModelMessage::system("stay concise"),
        ModelMessage::user("hello"),
        ModelMessage::assistant("hi"),
    ];

    agent
        .replace_messages(history.clone())
        .await
        .expect("history should import");

    let thread = agent.read_snapshot().await.threads[0].clone();

    assert_eq!(agent.messages().await, history);
    assert_eq!(
        thread
            .messages
            .iter()
            .map(|message| message.payload.clone())
            .collect::<Vec<_>>(),
        history
    );
    assert!(
        thread.revision > initial.revision,
        "history import should advance revision"
    );
    assert!(thread.active_turn_id.is_none());
    assert_eq!(thread.turns.len(), 1);
    assert!(thread
        .turns
        .iter()
        .all(|turn| turn.status == TurnStatus::Completed));
    assert!(thread
        .messages
        .iter()
        .all(|message| message.status == MessageStatus::Completed));
}

#[tokio::test]
async fn reset_clears_chat_messages_and_invalidates_prior_ids() {
    let agent = runtime_with_chat_provider();
    agent
        .replace_messages(vec![ModelMessage::user("old")])
        .await
        .expect("history should import");
    let before = agent.read_snapshot().await.threads[0].clone();
    let old_message_id = before.messages[0].message_id;

    agent.reset().await;

    let after = agent.read_snapshot().await.threads[0].clone();

    assert!(agent.messages().await.is_empty());
    assert!(after.messages.is_empty());
    assert!(after.turns.is_empty());
    assert!(after.tools.is_empty());
    assert!(after.active_turn_id.is_none());
    assert!(
        after.revision > before.revision || after.last_seq > before.last_seq,
        "reset should make prior ids stale-compatible"
    );
    assert!(
        after.revision != old_message_id.revision() || after.last_seq > before.last_seq,
        "old message ids should not look current after reset"
    );
}
