use std::fs;

use tempfile::tempdir;

use super::chat::{
    AgentRuntimeEvent, AgentRuntimeEventPayload, AgentRuntimeEventStore,
    JsonlAgentRuntimeEventStore, ThreadId, TurnId, TurnSnapshot, TurnStatus,
};
use crate::session::recovery::SessionRecoverySource;
use crate::session::{
    CreateSessionOptions, LocalProviderLedger, LocalSessionStore, ProviderLedgerRecord,
    SessionConfig, SessionError, SessionId,
};
use crate::types::ModelMessage;

fn session_id(value: &str) -> SessionId {
    SessionId::parse(value).expect("session id should parse")
}

fn test_event(thread_id: ThreadId, seq: u64) -> AgentRuntimeEvent {
    let now = chrono::Utc::now();
    let turn = TurnSnapshot {
        turn_id: TurnId::new(thread_id, 0, seq),
        thread_id,
        status: TurnStatus::Queued,
        message_ids: Vec::new(),
        active_tool_call_ids: Vec::new(),
        error: None,
        queued_at: now,
        started_at: None,
        completed_at: None,
    };

    AgentRuntimeEvent::new(
        seq,
        thread_id,
        Some(turn.turn_id),
        AgentRuntimeEventPayload::TurnQueued { turn },
    )
}

fn event_record(event: AgentRuntimeEvent) -> String {
    serde_json::json!({
        "type": "event",
        "event": event,
    })
    .to_string()
}

fn default_thread_after_other() -> (ThreadId, ThreadId) {
    loop {
        let default_thread_id = ThreadId::new();
        let other_thread_id = ThreadId::new();
        if default_thread_id.to_string() > other_thread_id.to_string() {
            return (default_thread_id, other_thread_id);
        }
    }
}

async fn create_recoverable_session(
    store: &LocalSessionStore,
    id: SessionId,
) -> (ThreadId, std::path::PathBuf) {
    let state = store
        .create(CreateSessionOptions {
            id: Some(id),
            ..CreateSessionOptions::default()
        })
        .await
        .expect("session should create");
    let thread_id = state.default_thread_id;
    let conventions = state.session_config.conventions();
    drop(state);

    let event_store = JsonlAgentRuntimeEventStore::open(conventions.events_file())
        .expect("event store should open");
    event_store
        .append(test_event(thread_id, 1))
        .await
        .expect("event should append");
    (thread_id, conventions.root().to_path_buf())
}

#[tokio::test]
async fn recover_import_rejects_plain_snapshot_and_existing_target() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let source_id = session_id("recover-source");
    create_recoverable_session(&store, source_id.clone()).await;
    let recovered = store
        .recover_export(SessionRecoverySource::SessionId(source_id))
        .await
        .expect("session should recover");

    let target_id = session_id("recover-target");
    let mut plain_snapshot_artifact = recovered.clone();
    plain_snapshot_artifact.artifact_type = "session_snapshot".to_string();
    let plain_err = store
        .recover_import(plain_snapshot_artifact, target_id.clone())
        .await
        .expect_err("plain snapshot artifact should be rejected");
    assert!(matches!(
        plain_err,
        SessionError::InvalidRecoveredSession { .. }
    ));
    assert!(!sessions.path().join(target_id.as_str()).exists());

    let existing = store
        .create(CreateSessionOptions {
            id: Some(target_id.clone()),
            ..CreateSessionOptions::default()
        })
        .await
        .expect("target session should create");
    drop(existing);

    let existing_err = store
        .recover_import(recovered, target_id)
        .await
        .expect_err("existing target should be rejected");
    assert!(matches!(existing_err, SessionError::AlreadyExists { .. }));
}

#[tokio::test]
async fn recover_import_leaves_no_target_on_tampered_artifact() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let source_id = session_id("recover-tamper-source");
    create_recoverable_session(&store, source_id.clone()).await;
    let mut recovered = store
        .recover_export(SessionRecoverySource::SessionId(source_id))
        .await
        .expect("session should recover");
    recovered.report.importable_runtime_state = false;
    let target_id = session_id("recover-tamper-target");

    let err = store
        .recover_import(recovered, target_id.clone())
        .await
        .expect_err("tampered artifact should be rejected");

    assert!(matches!(err, SessionError::NonImportableRecovery { .. }));
    assert!(!sessions.path().join(target_id.as_str()).exists());
}

#[tokio::test]
async fn corrupt_provider_ledger_still_breaks_normal_open() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let session_id = session_id("recover-corrupt-provider");
    let state = store
        .create(CreateSessionOptions {
            id: Some(session_id.clone()),
            ..CreateSessionOptions::default()
        })
        .await
        .expect("session should create");
    let conventions = state.session_config.conventions();
    drop(state);
    fs::write(conventions.provider_ledger_file(), "not-json\n")
        .expect("provider ledger should be corrupted");

    let err = store
        .open(session_id)
        .await
        .expect_err("corrupt provider ledger should fail normal open");
    assert!(err.to_string().contains("model_messages.jsonl"));
}

#[tokio::test]
async fn corrupt_events_still_break_normal_open_and_export() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let session_id = session_id("recover-corrupt-events");
    let state = store
        .create(CreateSessionOptions {
            id: Some(session_id.clone()),
            ..CreateSessionOptions::default()
        })
        .await
        .expect("session should create");
    let conventions = state.session_config.conventions();
    drop(state);
    fs::write(conventions.events_file(), "not-json\n").expect("events jsonl should be corrupted");

    let open_err = store
        .open(session_id.clone())
        .await
        .expect_err("corrupt events should fail normal open");
    assert!(open_err.to_string().contains("events.jsonl"));

    let export_err = store
        .export_snapshot(session_id)
        .await
        .expect_err("corrupt events should fail normal export");
    assert!(export_err.to_string().contains("events.jsonl"));
}

#[tokio::test]
async fn recovery_export_accepts_final_event_without_newline() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let session_id = session_id("recover-final-newline");
    let state = store
        .create(CreateSessionOptions {
            id: Some(session_id.clone()),
            ..CreateSessionOptions::default()
        })
        .await
        .expect("session should create");
    let thread_id = state.default_thread_id;
    let conventions = state.session_config.conventions();
    drop(state);
    fs::write(
        conventions.events_file(),
        event_record(test_event(thread_id, 1)),
    )
    .expect("events jsonl should be written without trailing newline");

    let recovered = store
        .recover_export(SessionRecoverySource::SessionId(session_id))
        .await
        .expect("session should recover");

    assert_eq!(recovered.snapshot.events.len(), 1);
    assert!(recovered.report.importable_runtime_state);
    assert!(recovered
        .report
        .warnings
        .iter()
        .any(|warning| warning.code == "events_final_line_missing_newline"));
}

#[tokio::test]
async fn recovery_export_raw_session_dir_derives_source_id_from_basename() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let source_id = session_id("recover-raw-basename");
    let (_thread_id, session_dir) = create_recoverable_session(&store, source_id.clone()).await;
    fs::write(session_dir.join("metadata.json"), "not-json")
        .expect("metadata json should be corrupted");

    let recovered = store
        .recover_export(SessionRecoverySource::SessionDir {
            path: session_dir,
            source_id: None,
        })
        .await
        .expect("raw session dir should recover from basename id");

    assert_eq!(recovered.snapshot.metadata.id, source_id);
    assert!(recovered.report.importable_runtime_state);
    assert!(recovered
        .report
        .warnings
        .iter()
        .any(|warning| warning.code == "metadata_unusable"));
}

#[tokio::test]
async fn recovery_export_uses_first_recovered_event_thread_when_cache_missing() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let session_id = session_id("recover-missing-cache");
    let (thread_id, _root) = create_recoverable_session(&store, session_id.clone()).await;
    let conventions = SessionConfig::new(session_id.clone(), sessions.path()).conventions();
    fs::remove_file(conventions.runtime_snapshot_file())
        .expect("runtime snapshot cache should be removed");
    let ledger = LocalProviderLedger::open(conventions.provider_ledger_file())
        .expect("provider ledger should open");
    ledger
        .append_message(thread_id, ModelMessage::user("default provider context"))
        .expect("provider message should append");
    drop(ledger);

    let recovered = store
        .recover_export(SessionRecoverySource::SessionId(session_id))
        .await
        .expect("session should recover without runtime cache");

    assert_eq!(recovered.snapshot.default_thread_id, thread_id);
    assert_eq!(recovered.snapshot.provider_ledger.thread_id, thread_id);
    assert_eq!(
        recovered.snapshot.provider_ledger.effective_history[0].text(),
        "default provider context"
    );
}

#[tokio::test]
async fn recovery_export_uses_unique_provider_thread_when_cache_and_events_missing() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let session_id = session_id("recover-ledger-only");
    let state = store
        .create(CreateSessionOptions {
            id: Some(session_id.clone()),
            ..CreateSessionOptions::default()
        })
        .await
        .expect("session should create");
    let thread_id = state.default_thread_id;
    let conventions = state.session_config.conventions();
    drop(state);
    fs::remove_file(conventions.runtime_snapshot_file())
        .expect("runtime snapshot cache should be removed");

    let ledger = LocalProviderLedger::open(conventions.provider_ledger_file())
        .expect("provider ledger should open");
    ledger
        .append_message(thread_id, ModelMessage::user("provider-only"))
        .expect("provider message should append");
    drop(ledger);

    let recovered = store
        .recover_export(SessionRecoverySource::SessionId(session_id))
        .await
        .expect("ledger-only session should recover");

    assert!(recovered.report.importable_runtime_state);
    assert_eq!(recovered.snapshot.default_thread_id, thread_id);
    assert_eq!(recovered.snapshot.provider_ledger.thread_id, thread_id);
    assert_eq!(
        recovered.snapshot.provider_ledger.effective_history[0].text(),
        "provider-only"
    );
}

#[tokio::test]
async fn recovery_export_rejects_ambiguous_provider_only_default_thread() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let session_id = session_id("recover-ledger-ambiguous");
    let state = store
        .create(CreateSessionOptions {
            id: Some(session_id.clone()),
            ..CreateSessionOptions::default()
        })
        .await
        .expect("session should create");
    let conventions = state.session_config.conventions();
    drop(state);
    fs::remove_file(conventions.runtime_snapshot_file())
        .expect("runtime snapshot cache should be removed");

    let first_thread_id = ThreadId::new();
    let second_thread_id = ThreadId::new();
    let ledger = LocalProviderLedger::open(conventions.provider_ledger_file())
        .expect("provider ledger should open");
    ledger
        .append_message(first_thread_id, ModelMessage::user("first"))
        .expect("first provider message should append");
    ledger
        .append_message(second_thread_id, ModelMessage::user("second"))
        .expect("second provider message should append");
    drop(ledger);

    let recovered = store
        .recover_export(SessionRecoverySource::SessionId(session_id))
        .await
        .expect("ambiguous ledger-only session should recover diagnostically");

    assert!(!recovered.report.importable_runtime_state);
    assert!(recovered
        .report
        .warnings
        .iter()
        .any(|warning| warning.code == "provider_default_thread_ambiguous"));
    assert_eq!(recovered.report.provider_context.recovered_threads.len(), 2);
}

#[tokio::test]
async fn recovery_export_recovers_provider_compacted_checkpoint_and_suffix() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let session_id = session_id("recover-provider-history");
    let (thread_id, _root) = create_recoverable_session(&store, session_id.clone()).await;
    let conventions = SessionConfig::new(session_id.clone(), sessions.path()).conventions();
    let ledger = LocalProviderLedger::open(conventions.provider_ledger_file())
        .expect("provider ledger should open");
    ledger
        .append_message(thread_id, ModelMessage::user("old"))
        .expect("provider message should append");
    ledger
        .append_compacted(thread_id, vec![ModelMessage::user("checkpoint")])
        .expect("provider compaction should append");
    ledger
        .append_message(thread_id, ModelMessage::assistant("suffix"))
        .expect("provider suffix should append");
    drop(ledger);

    let recovered = store
        .recover_export(SessionRecoverySource::SessionId(session_id))
        .await
        .expect("session should recover");
    let texts = recovered
        .snapshot
        .provider_ledger
        .effective_history
        .iter()
        .map(|message| message.text().to_string())
        .collect::<Vec<_>>();

    assert_eq!(texts, vec!["checkpoint", "suffix"]);
    assert!(recovered.report.importable_runtime_state);
}

#[tokio::test]
async fn recovery_import_writes_openable_session_with_compacted_provider_ledger() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let source_id = session_id("recover-import-source");
    let (thread_id, _root) = create_recoverable_session(&store, source_id.clone()).await;
    let conventions = SessionConfig::new(source_id.clone(), sessions.path()).conventions();
    let ledger = LocalProviderLedger::open(conventions.provider_ledger_file())
        .expect("provider ledger should open");
    ledger
        .append_message(thread_id, ModelMessage::user("kept"))
        .expect("provider message should append");
    drop(ledger);
    let recovered = store
        .recover_export(SessionRecoverySource::SessionId(source_id))
        .await
        .expect("session should recover");
    let target_id = session_id("recover-import-target");

    let imported = store
        .recover_import(recovered, target_id.clone())
        .await
        .expect("recovered session should import");
    drop(imported);

    let target_conventions = SessionConfig::new(target_id.clone(), sessions.path()).conventions();
    assert!(target_conventions.files_dir().is_dir());
    assert!(target_conventions.artifacts_dir().is_dir());
    assert!(target_conventions.temp_dir().is_dir());
    assert!(target_conventions.checkpoints_dir().is_dir());

    let provider_path = SessionConfig::new(target_id.clone(), sessions.path())
        .conventions()
        .provider_ledger_file();
    let records = fs::read_to_string(provider_path).expect("provider ledger should be readable");
    let record_count = records.lines().count();
    let first_record: ProviderLedgerRecord =
        serde_json::from_str(records.lines().next().expect("record should exist"))
            .expect("provider record should deserialize");
    assert_eq!(record_count, 1);
    assert!(matches!(
        first_record,
        ProviderLedgerRecord::Compacted { .. }
    ));

    let reopened = store
        .open(target_id)
        .await
        .expect("imported recovered session should open");
    assert_eq!(reopened.model_messages.len(), 1);
    assert_eq!(reopened.model_messages[0].text(), "kept");
}

#[tokio::test]
async fn recovery_import_uses_artifact_default_thread_for_multithread_provider_context() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let source_id = session_id("recover-multithread-source");
    let (default_thread_id, other_thread_id) = default_thread_after_other();
    let state = store
        .create(CreateSessionOptions {
            id: Some(source_id.clone()),
            default_thread_id: Some(default_thread_id),
            ..CreateSessionOptions::default()
        })
        .await
        .expect("session should create");
    let conventions = state.session_config.conventions();
    drop(state);

    let event_store = JsonlAgentRuntimeEventStore::open(conventions.events_file())
        .expect("event store should open");
    event_store
        .append(test_event(other_thread_id, 1))
        .await
        .expect("other thread event should append");
    event_store
        .append(test_event(default_thread_id, 1))
        .await
        .expect("default thread event should append");
    let ledger = LocalProviderLedger::open(conventions.provider_ledger_file())
        .expect("provider ledger should open");
    ledger
        .append_message(other_thread_id, ModelMessage::user("other"))
        .expect("other provider message should append");
    ledger
        .append_message(default_thread_id, ModelMessage::user("default"))
        .expect("default provider message should append");
    drop(ledger);

    let recovered = store
        .recover_export(SessionRecoverySource::SessionId(source_id))
        .await
        .expect("session should recover");
    assert_eq!(recovered.snapshot.default_thread_id, default_thread_id);
    assert_eq!(
        recovered.snapshot.provider_ledger.thread_id,
        default_thread_id
    );
    assert_eq!(
        recovered.snapshot.provider_ledger.effective_history[0].text(),
        "default"
    );

    let target_id = session_id("recover-multithread-target");
    let imported = store
        .recover_import(recovered, target_id)
        .await
        .expect("recovered multithread session should import");

    assert_eq!(imported.default_thread_id, default_thread_id);
    assert_eq!(imported.model_messages.len(), 1);
    assert_eq!(imported.model_messages[0].text(), "default");
}
