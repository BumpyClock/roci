use std::fs;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;

use tempfile::tempdir;
use tokio::time::{timeout, Duration};

use super::chat::{AgentRuntimeEvent, AgentRuntimeEventPayload, TurnStatus};
use super::support::*;
use super::*;
use crate::session::{LogicalPath, SessionConfig, SessionId};

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

fn runtime_with_session(root: &std::path::Path, id: &str) -> AgentRuntime {
    let registry = registry_with_streaming_provider("stub", 1, 1);
    let mut config = test_agent_config();
    config.model = "stub:session".parse().expect("stub model should parse");
    config.session = Some(SessionConfig::new(session_id(id), root));
    AgentRuntime::try_new(registry, test_config(), config).expect("runtime should construct")
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

#[tokio::test(flavor = "current_thread")]
async fn session_config_uses_jsonl_store_without_project_cwd_storage() {
    let project = tempdir().expect("project tempdir should be created");
    let sessions = tempdir().expect("session tempdir should be created");
    let session_id = session_id("session-jsonl");
    let agent = {
        let _cwd_lock = CWD_LOCK.lock().expect("cwd lock should not be poisoned");
        let _cwd_guard = CwdGuard::change_to(project.path().to_path_buf());
        let registry = registry_with_streaming_provider("stub", 1, 1);
        let mut config = test_agent_config();
        config.model = "stub:session".parse().expect("stub model should parse");
        config.session = Some(SessionConfig::new(session_id.clone(), sessions.path()));
        AgentRuntime::try_new(registry, test_config(), config).expect("runtime should construct")
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
    config.model = "plan-json:stub-model"
        .parse()
        .expect("stub model should parse");
    config.session = Some(SessionConfig::new(session_id.clone(), sessions.path()));
    let agent =
        AgentRuntime::try_new(registry, test_config(), config).expect("runtime should construct");

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

#[test]
fn session_constructor_surfaces_corrupt_events_jsonl() {
    let sessions = tempdir().expect("session tempdir should be created");
    let session_id = session_id("session-corrupt");
    let conventions = SessionConfig::new(session_id.clone(), sessions.path()).conventions();
    fs::create_dir_all(conventions.root()).expect("session dir should be created");
    fs::write(conventions.events_file(), "not-json\n").expect("events jsonl should be written");
    let registry = registry_with_streaming_provider("stub", 1, 1);
    let mut config = test_agent_config();
    config.model = "stub:session".parse().expect("stub model should parse");
    config.session = Some(SessionConfig::new(session_id, sessions.path()));

    let err = match AgentRuntime::try_new(registry, test_config(), config) {
        Ok(_) => panic!("corrupt events.jsonl should fail constructor"),
        Err(err) => err,
    };
    let message = err.to_string();
    assert!(message.contains("events.jsonl"));
    assert!(message.contains("line 1"));
}

#[tokio::test]
async fn runtime_resource_methods_write_files_events_and_snapshot() {
    let sessions = tempdir().expect("session tempdir should be created");
    let session_id = session_id("session-resources");
    let agent = runtime_with_session(sessions.path(), session_id.as_str());
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
