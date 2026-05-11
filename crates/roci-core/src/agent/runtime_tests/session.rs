use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use tempfile::tempdir;
use tokio::time::{timeout, Duration};

use super::chat::{AgentRuntimeEvent, AgentRuntimeEventPayload, TurnStatus};
use super::support::*;
use super::*;
use crate::session::{
    CreateSessionOptions, ImportPolicy, LocalProviderLedger, LocalSessionResources,
    LocalSessionStore, LogicalPath, SessionConfig, SessionId,
};

static CWD_LOCK: StdMutex<()> = StdMutex::new(());

struct CwdGuard {
    original: PathBuf,
}

impl CwdGuard {
    fn change_to(path: PathBuf) -> Self {
        let original = std::env::current_dir().expect("current dir should be readable");
        std::env::set_current_dir(&path).expect("cwd should change");
        Self { original }
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
    }
}

fn session_id(value: &str) -> SessionId {
    SessionId::parse(value).expect("session id should parse")
}

fn logical_path(value: &str) -> LogicalPath {
    LogicalPath::parse(value).expect("logical path should parse")
}

async fn runtime_with_session(root: &std::path::Path, id: &str) -> AgentRuntime {
    let registry = registry_with_streaming_provider("stub", 1, 1);
    let mut config = test_agent_config();
    config.candidates = vec!["stub:session".parse().expect("stub model should parse")];
    let store = LocalSessionStore::new(root);
    let state = store
        .create(CreateSessionOptions {
            id: Some(session_id(id)),
            ..CreateSessionOptions::default()
        })
        .await
        .expect("session should create");
    AgentRuntime::resume_session(registry, test_config(), config, state)
        .await
        .expect("runtime should resume")
}

async fn recv_event(sub: &mut RuntimeSubscription) -> AgentRuntimeEvent {
    timeout(Duration::from_secs(2), sub.recv())
        .await
        .expect("event should arrive before timeout")
        .expect("event stream should be open")
}

async fn assert_turn_status(agent: &AgentRuntime, turn_id: TurnId, status: TurnStatus) {
    agent.wait_for_idle().await;
    let thread = agent
        .read_thread(agent.default_thread_id())
        .await
        .expect("thread should be readable");
    let turn = thread
        .turns
        .iter()
        .find(|turn| turn.turn_id == turn_id)
        .expect("turn should exist");
    assert_eq!(turn.status, status);
}

#[tokio::test]
async fn local_session_store_create_writes_metadata_once_and_open_preserves_it() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-store");
    let state = store
        .create(CreateSessionOptions {
            id: Some(id.clone()),
            title: Some("Store test".to_string()),
            host_cwd: Some(PathBuf::from("/tmp/project")),
            import_source: None,
            default_thread_id: None,
        })
        .await
        .expect("session creates");
    let created_at = state.metadata.created_at;
    drop(state);

    let reopened = store.open(id.clone()).await.expect("session opens");

    assert_eq!(reopened.metadata.id, id);
    assert_eq!(reopened.metadata.title.as_deref(), Some("Store test"));
    assert_eq!(reopened.metadata.created_at, created_at);
    assert_eq!(
        reopened.metadata.host_cwd,
        Some(PathBuf::from("/tmp/project"))
    );
    assert!(
        SessionConfig::new(reopened.metadata.id.clone(), sessions.path())
            .conventions()
            .provider_ledger_file()
            .is_file()
    );
}

#[tokio::test]
async fn local_session_store_rejects_second_open_while_resume_state_alive() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-lease");
    let _state = store
        .create(CreateSessionOptions {
            id: Some(id.clone()),
            ..CreateSessionOptions::default()
        })
        .await
        .unwrap();

    let err = store.open(id).await.unwrap_err();

    assert!(err.to_string().contains("already open"));
}

#[tokio::test]
async fn resume_session_seeds_runtime_snapshot_and_provider_ledger() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-resume");
    let created = store
        .create(CreateSessionOptions {
            id: Some(id.clone()),
            ..CreateSessionOptions::default()
        })
        .await
        .unwrap();
    let thread_id = created.default_thread_id;
    let provider_ledger_file = created.session_config.conventions().provider_ledger_file();
    drop(created);
    let ledger = LocalProviderLedger::open(provider_ledger_file).unwrap();
    let persisted = ModelMessage::user("persisted");
    ledger
        .append_message(thread_id, persisted.clone())
        .expect("ledger appends");
    drop(ledger);
    let state = store.open(id).await.unwrap();
    let registry = registry_with_streaming_provider("stub", 1, 1);
    let mut config = test_agent_config();
    config.candidates = vec!["stub:session".parse().unwrap()];

    let agent = AgentRuntime::resume_session(registry, test_config(), config, state)
        .await
        .expect("session resumes");

    assert_eq!(agent.messages().await, vec![persisted]);
}

#[tokio::test]
async fn completed_turn_appends_provider_ledger_messages() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-ledger-turn");
    let state = store
        .create(CreateSessionOptions {
            id: Some(id.clone()),
            ..CreateSessionOptions::default()
        })
        .await
        .unwrap();
    let registry = registry_with_streaming_provider("stub", 1, 1);
    let mut config = test_agent_config();
    config.candidates = vec!["stub:session".parse().unwrap()];
    let agent = AgentRuntime::resume_session(registry, test_config(), config, state)
        .await
        .unwrap();

    let turn_id = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("persist me")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: None,
        })
        .await
        .unwrap();
    assert_turn_status(&agent, turn_id, TurnStatus::Completed).await;
    drop(agent);

    let reopened = store.open(id).await.unwrap();
    assert_eq!(reopened.model_messages.len(), 2);
    assert_eq!(reopened.model_messages[0].role, Role::User);
    assert_eq!(reopened.model_messages[0].text(), "persist me");
    assert_eq!(reopened.model_messages[1].role, Role::Assistant);
    assert_eq!(reopened.model_messages[1].text(), "hello");
}

#[tokio::test]
async fn replace_messages_writes_compacted_provider_ledger() {
    let sessions = tempdir().unwrap();
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-ledger-replace");
    let state = store
        .create(CreateSessionOptions {
            id: Some(id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    let agent =
        AgentRuntime::resume_session(test_registry(), test_config(), test_agent_config(), state)
            .await
            .unwrap();

    let replacement = ModelMessage::user("replacement");
    agent
        .replace_messages(vec![replacement.clone()])
        .await
        .unwrap();
    drop(agent);

    let reopened = store.open(id).await.unwrap();
    assert_eq!(reopened.model_messages, vec![replacement]);
}

#[tokio::test]
async fn replace_messages_after_resume_compacts_provider_ledger_without_suffix_panic() {
    let sessions = tempdir().unwrap();
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-ledger-resume-replace");
    let state = store
        .create(CreateSessionOptions {
            id: Some(id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    let thread_id = state.default_thread_id;
    let ledger_path = state.session_config.conventions().provider_ledger_file();
    drop(state);
    let ledger = LocalProviderLedger::open(ledger_path).unwrap();
    ledger
        .append_message(thread_id, ModelMessage::user("old 1"))
        .unwrap();
    ledger
        .append_message(thread_id, ModelMessage::assistant("old 2"))
        .unwrap();
    drop(ledger);

    let state = store.open(id.clone()).await.unwrap();
    let agent =
        AgentRuntime::resume_session(test_registry(), test_config(), test_agent_config(), state)
            .await
            .unwrap();
    let replacement = ModelMessage::user("short replacement");
    agent
        .replace_messages(vec![replacement.clone()])
        .await
        .unwrap();
    drop(agent);

    let reopened = store.open(id).await.unwrap();

    assert_eq!(reopened.model_messages, vec![replacement]);
}

#[tokio::test]
async fn compact_writes_compacted_provider_ledger_for_resume() {
    let sessions = tempdir().unwrap();
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-ledger-compact");
    let state = store
        .create(CreateSessionOptions {
            id: Some(id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    let registry = registry_with_summary_provider(
        "stub",
        "durable compact summary",
        Arc::new(StdMutex::new(Vec::new())),
    );
    let mut config = test_agent_config();
    config.candidates = vec!["stub:run-model".parse().unwrap()];
    config.compaction.keep_recent_tokens = 1;
    let agent = AgentRuntime::resume_session(registry, test_config(), config, state)
        .await
        .unwrap();
    agent
        .replace_messages(vec![
            ModelMessage::user("old user"),
            ModelMessage::assistant("old answer"),
            ModelMessage::user("latest request"),
        ])
        .await
        .unwrap();

    agent.compact().await.unwrap();
    let compacted = agent.messages().await;
    assert!(compacted
        .iter()
        .any(|message| message.text().contains("durable compact summary")));
    drop(agent);

    let reopened = store.open(id).await.unwrap();
    assert_eq!(reopened.model_messages, compacted);
}

#[tokio::test]
async fn export_snapshot_contains_manifest_and_no_resource_bytes() {
    let sessions = tempdir().unwrap();
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-export");
    let state = store
        .create(CreateSessionOptions {
            id: Some(id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    let resources =
        LocalSessionResources::with_conventions(state.session_config.conventions()).unwrap();
    resources
        .write_artifact(logical_path("artifact.txt"), b"payload bytes")
        .unwrap();
    drop(state);

    let snapshot = store.export_snapshot(id).await.unwrap();
    let json = serde_json::to_string(&snapshot).unwrap();

    assert!(json.contains("artifact.txt"));
    assert!(!json.contains("payload bytes"));
}

#[tokio::test]
async fn import_snapshot_new_id_preserves_unavailable_resource_manifest() {
    let sessions = tempdir().unwrap();
    let source_store = LocalSessionStore::new(sessions.path().join("source"));
    let target_store = LocalSessionStore::new(sessions.path().join("target"));
    let source_id = session_id("source-session");
    let source = source_store
        .create(CreateSessionOptions {
            id: Some(source_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    LocalSessionResources::with_conventions(source.session_config.conventions())
        .unwrap()
        .write_artifact(logical_path("artifact.txt"), b"payload")
        .unwrap();
    drop(source);
    let snapshot = source_store.export_snapshot(source_id).await.unwrap();
    let target_id = session_id("target-session");

    let imported = target_store
        .import_snapshot(snapshot, ImportPolicy::NewId(Some(target_id.clone())))
        .await
        .unwrap();

    assert_eq!(imported.metadata.id, target_id);
    assert_eq!(imported.resources.artifacts.len(), 1);
    assert!(!imported.resources.artifacts[0].available);
    assert!(imported
        .runtime
        .threads
        .iter()
        .all(|thread| thread.resources.artifacts.is_empty()));
}

#[tokio::test(flavor = "current_thread")]
async fn session_config_uses_jsonl_store_without_project_cwd_storage() {
    let project = tempdir().expect("project tempdir should be created");
    let sessions = tempdir().expect("session tempdir should be created");
    let session_id = session_id("session-jsonl");
    let host_cwd = {
        let _cwd_lock = CWD_LOCK.lock().expect("cwd lock should not be poisoned");
        let _cwd_guard = CwdGuard::change_to(project.path().to_path_buf());
        std::env::current_dir().ok()
    };
    let agent = {
        let registry = registry_with_streaming_provider("stub", 1, 1);
        let mut config = test_agent_config();
        config.candidates = vec!["stub:session".parse().expect("stub model should parse")];
        let store = LocalSessionStore::new(sessions.path());
        let state = store
            .create(CreateSessionOptions {
                id: Some(session_id.clone()),
                host_cwd,
                ..CreateSessionOptions::default()
            })
            .await
            .expect("session should create");
        AgentRuntime::resume_session(registry, test_config(), config, state)
            .await
            .expect("runtime should resume")
    };

    let turn_id = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("hello")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: None,
        })
        .await
        .expect("turn should queue");
    assert_turn_status(&agent, turn_id, TurnStatus::Completed).await;

    let conventions = SessionConfig::new(session_id.clone(), sessions.path()).conventions();
    assert!(conventions.events_file().is_file());
    assert!(conventions.metadata_file().is_file());
    assert!(!project.path().join(".roci").exists());
    assert!(!project.path().join(session_id.as_str()).exists());
}

#[tokio::test]
async fn plan_updates_are_mirrored_to_plan_md() {
    let sessions = tempdir().expect("session tempdir should be created");
    let session_id = session_id("session-plan");
    let registry = registry_with_plan_json_provider("plan-json", "make a plan");
    let mut config = test_agent_config();
    config.candidates = vec!["plan-json:stub-model"
        .parse()
        .expect("stub model should parse")];
    let store = LocalSessionStore::new(sessions.path());
    let state = store
        .create(CreateSessionOptions {
            id: Some(session_id.clone()),
            ..CreateSessionOptions::default()
        })
        .await
        .expect("session should create");
    let agent = AgentRuntime::resume_session(registry, test_config(), config, state)
        .await
        .expect("runtime should resume");

    let turn_id = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("make a plan")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: Some(CollaborationMode::Plan),
        })
        .await
        .expect("turn should queue");
    assert_turn_status(&agent, turn_id, TurnStatus::Completed).await;

    let plan = fs::read_to_string(
        SessionConfig::new(session_id, sessions.path())
            .conventions()
            .plan_file(),
    )
    .expect("plan.md should be readable");
    assert!(plan.contains("make a plan"));
}

#[tokio::test]
async fn session_store_open_surfaces_corrupt_events_jsonl() {
    let sessions = tempdir().expect("session tempdir should be created");
    let session_id = session_id("session-corrupt");
    let store = LocalSessionStore::new(sessions.path());
    let state = store
        .create(CreateSessionOptions {
            id: Some(session_id.clone()),
            ..CreateSessionOptions::default()
        })
        .await
        .expect("session should create");
    let conventions = state.session_config.conventions();
    drop(state);
    fs::write(conventions.events_file(), "not-json\n").expect("events jsonl should be written");

    let err = store
        .open(session_id)
        .await
        .expect_err("corrupt events.jsonl should fail open");
    let message = err.to_string();
    assert!(message.contains("events.jsonl"));
    assert!(message.contains("line 1"));
}

#[tokio::test]
async fn runtime_resource_methods_write_files_events_and_snapshot() {
    let sessions = tempdir().expect("session tempdir should be created");
    let session_id = session_id("session-resources");
    let agent = runtime_with_session(sessions.path(), session_id.as_str()).await;
    let conventions = SessionConfig::new(session_id.clone(), sessions.path()).conventions();

    let mut sub = agent.subscribe(None).await;
    let workspace = agent
        .write_workspace_yaml("cwd: files\n")
        .await
        .expect("workspace should be written");
    assert_eq!(
        fs::read_to_string(conventions.workspace_file()).expect("workspace should be readable"),
        "cwd: files\n"
    );
    let event = recv_event(&mut sub).await;
    assert!(matches!(
        event.payload,
        AgentRuntimeEventPayload::WorkspaceUpdated { resource } if resource == workspace
    ));
    assert_eq!(
        agent
            .read_thread(agent.default_thread_id())
            .await
            .unwrap()
            .resources
            .workspace,
        Some(workspace)
    );

    let artifact_path = logical_path("reports/out.txt");
    let mut sub = agent.subscribe(None).await;
    let artifact = agent
        .write_artifact(artifact_path.clone(), b"artifact")
        .await
        .expect("artifact should be written");
    assert_eq!(
        fs::read(conventions.artifact_path(&artifact_path)).expect("artifact should be readable"),
        b"artifact"
    );
    let event = recv_event(&mut sub).await;
    assert!(matches!(
        event.payload,
        AgentRuntimeEventPayload::ArtifactCreated { resource } if resource == artifact
    ));
    assert_eq!(
        agent
            .read_thread(agent.default_thread_id())
            .await
            .unwrap()
            .resources
            .artifacts,
        vec![artifact]
    );

    let temp_path = logical_path("scratch/cache.bin");
    let mut sub = agent.subscribe(None).await;
    let temp_file = agent
        .write_temp_file(temp_path.clone(), b"temp")
        .await
        .expect("temp file should be written");
    assert_eq!(
        fs::read(conventions.temp_path(&temp_path)).expect("temp file should be readable"),
        b"temp"
    );
    let event = recv_event(&mut sub).await;
    assert!(matches!(
        event.payload,
        AgentRuntimeEventPayload::TempFileWritten { resource } if resource == temp_file
    ));
    assert_eq!(
        agent
            .read_thread(agent.default_thread_id())
            .await
            .unwrap()
            .resources
            .temp_files,
        vec![temp_file]
    );

    let checkpoint_path = logical_path("turn-1/state.json");
    let mut sub = agent.subscribe(None).await;
    let checkpoint = agent
        .write_checkpoint(checkpoint_path.clone(), br#"{"ok":true}"#)
        .await
        .expect("checkpoint should be written");
    assert_eq!(
        fs::read(conventions.checkpoint_path(&checkpoint_path))
            .expect("checkpoint should be readable"),
        br#"{"ok":true}"#
    );
    let event = recv_event(&mut sub).await;
    assert!(matches!(
        event.payload,
        AgentRuntimeEventPayload::CheckpointCreated { resource } if resource == checkpoint
    ));
    assert_eq!(
        agent
            .read_thread(agent.default_thread_id())
            .await
            .unwrap()
            .resources
            .checkpoints,
        vec![checkpoint]
    );

    let file_path = logical_path("notes/today.txt");
    let mut sub = agent.subscribe(None).await;
    let file = agent
        .write_session_file(file_path.clone(), b"note")
        .await
        .expect("session file should be written");
    assert_eq!(
        fs::read(conventions.file_path(&file_path)).expect("session file should be readable"),
        b"note"
    );
    let event = recv_event(&mut sub).await;
    assert!(matches!(
        event.payload,
        AgentRuntimeEventPayload::SessionFileWritten { resource } if resource == file
    ));
    assert_eq!(
        agent
            .read_thread(agent.default_thread_id())
            .await
            .unwrap()
            .resources
            .files,
        vec![file.clone()]
    );

    let mut sub = agent.subscribe(None).await;
    let deleted = agent
        .delete_session_file(file_path.clone())
        .await
        .expect("session file should be deleted");
    assert!(!conventions.file_path(&file_path).exists());
    let event = recv_event(&mut sub).await;
    assert!(matches!(
        event.payload,
        AgentRuntimeEventPayload::SessionFileDeleted { resource } if resource == deleted
    ));
    assert!(agent
        .read_thread(agent.default_thread_id())
        .await
        .unwrap()
        .resources
        .files
        .is_empty());
}
