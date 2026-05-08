# Durable Session Surface Implementation Plan

> **For agentic workers:** Prefer `subagent-driven-development` for execution when available. Task implementers own task work and review fixes; integration owner owns final integration. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build opt-in durable session storage for strict runtime event replay, explicit session resources, and session-scoped tool execution.

**Architecture:** Host apps supply a durable session root; Roci never stores session data in project cwd by default. `JsonlAgentRuntimeEventStore` persists semantic runtime events exactly, `LocalSessionResources` owns resource files/events, and `ToolExecutionContext` carries optional session filesystem plus logical cwd for built-in tools.

**Tech Stack:** Rust 2021, `tokio`, `serde`, `serde_json`, `serde_yaml`, `chrono`, existing `roci-core` agent runtime/chat/session modules, existing `roci-tools` built-ins.

---

## File Structure

- Create `crates/roci-core/src/agent/runtime/chat/jsonl_store.rs`
  - Strict JSONL implementation of `AgentRuntimeEventStore`.
  - Owns JSONL record shape, replay loading, tombstone invalidation, and tests.
- Modify `crates/roci-core/src/agent/runtime/chat/mod.rs`
  - Export `JsonlAgentRuntimeEventStore`.
- Modify `crates/roci-core/src/agent/runtime/chat/error.rs`
  - Keep `ProjectionFailed` as corruption surface; add `corrupt_line_error` helper used by JSONL replay.
- Create `crates/roci-core/src/session/config.rs`
  - Define `SessionConfig { id, root, cwd }`.
- Create `crates/roci-core/src/session/resources.rs`
  - Define resource namespaces, metadata, `LocalSessionResources`, path validation, read/write/list/delete APIs.
- Modify `crates/roci-core/src/session/metadata.rs`
  - Add JSON read/write helpers for `metadata.json`.
- Modify `crates/roci-core/src/session/path.rs`
  - Add `plan_file()`, `workspace_file()`, and resource namespace path helpers.
- Modify `crates/roci-core/src/session/mod.rs`
  - Export config/resource APIs.
- Modify `crates/roci-core/src/prelude.rs`
  - Re-export stable session APIs.
- Modify `crates/roci-core/src/agent/runtime/chat/domain.rs`
  - Add resource snapshot DTOs.
- Modify `crates/roci-core/src/agent/runtime/chat/event.rs`
  - Add explicit resource event payloads.
- Modify `crates/roci-core/src/agent/runtime/chat/projector.rs`
  - Project explicit resource events into thread snapshots.
- Modify `crates/roci-core/src/agent/runtime/config.rs`
  - Add `AgentConfig.session: Option<SessionConfig>` and `AgentConfig.sandbox_provider`.
- Modify `crates/roci-core/src/agent/runtime.rs`
  - Wire session config into JSONL store, `LocalSessionFs`, resource handles, metadata init, and keep opt-in behavior.
- Modify `crates/roci-core/src/agent/runtime/events.rs`
  - Mirror `PlanUpdated` to `plan.md` and publish `PlanWritten`.
- Modify `crates/roci-core/src/agent/runtime/state.rs`
  - Add runtime session resource methods that write resources and publish explicit events.
- Modify `crates/roci-core/src/agent/runtime/run_loop.rs`
  - Mirror structured plan-mode updates to `plan.md`.
- Modify `crates/roci-core/src/tools/tool.rs`
  - Add optional session fields and shell sandbox seam to `ToolExecutionContext`.
- Modify `crates/roci-core/src/agent_loop/runner.rs`
  - Add optional session fields to `RunRequest`.
- Modify `crates/roci-core/src/agent_loop/runner/tooling.rs`
  - Populate `ToolExecutionContext` from `RunRequest`.
- Modify `crates/roci-core/src/agent/runtime/run_loop.rs`
  - Pass session fields into `RunRequest`.
- Modify `crates/roci-tools/src/builtin/{read_file,write_file,list_directory,grep,shell,common,tests}.rs`
  - Use session filesystem when present; preserve host behavior when absent.
- Add tests under `crates/roci-core/src/agent/runtime_tests/session.rs`
  - Runtime session/resource integration tests.
- Modify `crates/roci-core/src/agent/runtime_tests/mod.rs`
  - Include new `session` test module.
- Modify `crates/roci-core/src/agent/runtime_tests/support.rs`
  - Add streaming plan JSON provider helper for session plan-mode tests.
- Update docs:
  - `docs/agent-runtime-chat.md`
  - `docs/ARCHITECTURE.md`

---

## Task 0: Baseline Guard

**Files:**
- Read only.

- [ ] **Step 1: Confirm worktree before edits**

Run:

```bash
git status --short
```

Expected: existing dirty work may include `crates/roci-core/src/session/`, attachments, and subagent files. Do not revert unrelated changes.

- [ ] **Step 2: Run current narrow baselines**

Run:

```bash
cargo test -p roci-core session::
cargo test -p roci-core --features agent chat::store
cargo test -p roci-tools
```

Expected: all pass before implementation. If a baseline fails, stop and diagnose before changing behavior.

---

## Task 1: Strict JSONL Runtime Event Store

**Files:**
- Create: `crates/roci-core/src/agent/runtime/chat/jsonl_store.rs`
- Modify: `crates/roci-core/src/agent/runtime/chat/mod.rs`
- Test: `crates/roci-core/src/agent/runtime/chat/jsonl_store.rs`

- [ ] **Step 1: Write failing JSONL tests**

Add tests in the new module. Use the same helper shape as `store.rs` tests:

```rust
#[tokio::test]
async fn jsonl_store_appends_and_replays_events_after_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let store = JsonlAgentRuntimeEventStore::open(temp.path().join("events.jsonl"))
        .unwrap();
    let thread_id = ThreadId::new();

    store.append(test_event(thread_id, 1)).await.unwrap();
    store.append(test_event(thread_id, 2)).await.unwrap();
    drop(store);

    let reopened = JsonlAgentRuntimeEventStore::open(temp.path().join("events.jsonl"))
        .unwrap();
    let replay = reopened
        .events_after(RuntimeCursor::new(thread_id, 0))
        .await
        .unwrap();

    assert_eq!(replay.iter().map(|event| event.seq).collect::<Vec<_>>(), vec![1, 2]);
}

#[tokio::test]
async fn jsonl_store_records_invalidation_tombstones() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let store = JsonlAgentRuntimeEventStore::open(&path).unwrap();
    let thread_id = ThreadId::new();

    store.append(test_event(thread_id, 1)).await.unwrap();
    store.invalidate_thread(thread_id, 3).await.unwrap();
    drop(store);

    let reopened = JsonlAgentRuntimeEventStore::open(&path).unwrap();
    let err = reopened
        .events_after(RuntimeCursor::new(thread_id, 2))
        .await
        .unwrap_err();

    assert!(matches!(
        err,
        AgentRuntimeError::StaleRuntime {
            requested_seq: 2,
            oldest_available_seq: 4,
            latest_seq: 3,
            ..
        }
    ));
}

#[tokio::test]
async fn jsonl_store_rejects_corrupt_nonblank_committed_line() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    tokio::fs::write(&path, b"{\"type\":\"event\",\"event\":null}\nnot-json\n")
        .await
        .unwrap();

    let err = JsonlAgentRuntimeEventStore::open(&path).unwrap_err();

    assert!(matches!(err, AgentRuntimeError::ProjectionFailed { .. }));
    assert!(err.to_string().contains("events.jsonl"));
    assert!(err.to_string().contains("line 2"));
}

#[tokio::test]
async fn jsonl_store_ignores_blank_trailing_lines() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    tokio::fs::write(&path, b"\n").await.unwrap();

    JsonlAgentRuntimeEventStore::open(&path).unwrap();
}
```

- [ ] **Step 2: Run tests and confirm fail**

Run:

```bash
cargo test -p roci-core --features agent jsonl_store
```

Expected: compile failure because `JsonlAgentRuntimeEventStore` does not exist.

- [ ] **Step 3: Implement JSONL record and replay state**

Implement in `jsonl_store.rs`:

```rust
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum JsonlRuntimeRecord {
    Event { event: AgentRuntimeEvent },
    ThreadInvalidated { thread_id: ThreadId, latest_seq: u64 },
}

#[derive(Debug)]
pub struct JsonlAgentRuntimeEventStore {
    path: PathBuf,
    inner: tokio::sync::Mutex<JsonlEventStoreState>,
}

#[derive(Debug, Default)]
struct JsonlEventStoreState {
    threads: HashMap<ThreadId, ThreadEvents>,
}
```

Use sync `std::fs` in `open(path) -> Result<Self, AgentRuntimeError>` so `AgentRuntime::try_new` can build a store without async construction. Use `tokio::fs::OpenOptions` for async appends. Create parent dirs before first append/open.

- [ ] **Step 4: Implement strict parse error helper**

Return `AgentRuntimeError::ProjectionFailed` with path and line:

```rust
fn corrupt_line_error(path: &Path, line_number: usize, source: impl std::fmt::Display) -> AgentRuntimeError {
    AgentRuntimeError::ProjectionFailed {
        message: format!(
            "failed to replay runtime events from {} at line {}: {}",
            path.display(),
            line_number,
            source
        ),
    }
}
```

- [ ] **Step 5: Implement trait**

Implement `AgentRuntimeEventStore`:

```rust
#[async_trait]
impl AgentRuntimeEventStore for JsonlAgentRuntimeEventStore {
    async fn append(&self, event: AgentRuntimeEvent) -> Result<RuntimeCursor, AgentRuntimeError> {
        let mut inner = self.inner.lock().await;
        inner.validate_event(&event)?;
        self.append_record(JsonlRuntimeRecord::Event { event: event.clone() }).await?;
        inner.apply_event(event.clone())?;
        Ok(event.cursor())
    }

    async fn events_after(&self, cursor: RuntimeCursor) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
        self.inner.lock().await.events_after(cursor)
    }

    async fn invalidate_thread(&self, thread_id: ThreadId, latest_seq: u64) -> Result<(), AgentRuntimeError> {
        let mut inner = self.inner.lock().await;
        self.append_record(JsonlRuntimeRecord::ThreadInvalidated { thread_id, latest_seq }).await?;
        inner.apply_invalidation(thread_id, latest_seq);
        Ok(())
    }
}
```

Avoid holding a sync `Mutex` across `.await`; use `tokio::sync::Mutex`.
Do not mutate in-memory replay state until append+flush succeeds. A failed disk write must leave memory and disk consistent.

- [ ] **Step 6: Export store**

Modify `chat/mod.rs`:

```rust
pub mod jsonl_store;
pub use jsonl_store::JsonlAgentRuntimeEventStore;
```

- [ ] **Step 7: Verify**

Run:

```bash
cargo test -p roci-core --features agent jsonl_store
cargo test -p roci-core --features agent chat::store
```

Expected: all JSONL and in-memory store tests pass.

---

## Task 2: Session Config and Resource Files

**Files:**
- Create: `crates/roci-core/src/session/config.rs`
- Create: `crates/roci-core/src/session/resources.rs`
- Modify: `crates/roci-core/src/session/metadata.rs`
- Modify: `crates/roci-core/src/session/path.rs`
- Modify: `crates/roci-core/src/session/mod.rs`
- Modify: `crates/roci-core/src/prelude.rs`
- Test: `crates/roci-core/src/session/resources.rs`

- [ ] **Step 1: Write resource path tests**

Add tests in `resources.rs`:

```rust
#[test]
fn resources_write_plan_workspace_artifact_temp_and_checkpoint() {
    let temp = tempfile::tempdir().unwrap();
    let resources = LocalSessionResources::new(temp.path().join("session")).unwrap();

    let plan = resources.write_plan("# Plan\n").unwrap();
    let workspace = resources.write_workspace_yaml("files:\n  - README.md\n").unwrap();
    let artifact = resources.write_artifact(LogicalPath::parse("images/out.txt").unwrap(), b"artifact").unwrap();
    let temp_file = resources.write_temp(LogicalPath::parse("run/output.txt").unwrap(), b"tmp").unwrap();
    let checkpoint = resources.write_checkpoint(LogicalPath::parse("one/state.json").unwrap(), b"{}").unwrap();

    assert_eq!(std::fs::read_to_string(resources.conventions().plan_file()).unwrap(), "# Plan\n");
    assert!(resources.conventions().workspace_file().exists());
    assert!(resources.conventions().artifacts_dir().join("images/out.txt").exists());
    assert!(resources.conventions().temp_dir().join("run/output.txt").exists());
    assert!(resources.conventions().checkpoints_dir().join("one/state.json").exists());
    assert_eq!(plan.namespace, SessionResourceNamespace::Plan);
    assert_eq!(workspace.namespace, SessionResourceNamespace::Workspace);
    assert_eq!(artifact.len, 8);
    assert_eq!(temp_file.len, 3);
    assert_eq!(checkpoint.len, 2);
}

#[test]
fn resources_reject_escape_paths() {
    let temp = tempfile::tempdir().unwrap();
    let resources = LocalSessionResources::new(temp.path().join("session")).unwrap();

    assert!(LogicalPath::parse("../outside.txt").is_err());
    assert!(LogicalPath::parse("/tmp/out.txt").is_err());
    assert!(LogicalPath::parse("nested\\out.txt").is_err());
}

#[test]
fn metadata_json_is_written_under_session_root() {
    let temp = tempfile::tempdir().unwrap();
    let id = SessionId::parse("session-a").unwrap();
    let conventions = PathConventions::for_session(temp.path(), &id);
    let metadata = SessionMetadata::new(
        id.clone(),
        Some(PathBuf::from("/project/source")),
        None,
    );

    metadata.write_to_path(conventions.metadata_file()).unwrap();
    let loaded = SessionMetadata::read_from_path(conventions.metadata_file()).unwrap();

    assert_eq!(loaded.id, id);
    assert_eq!(loaded.host_cwd, Some(PathBuf::from("/project/source")));
}
```

- [ ] **Step 2: Run tests and confirm fail**

Run:

```bash
cargo test -p roci-core session::resources
```

Expected: compile failure because resource APIs do not exist.

- [ ] **Step 3: Add path conventions**

Modify `path.rs`:

```rust
pub fn plan_file(&self) -> PathBuf {
    self.root.join("plan.md")
}

pub fn workspace_file(&self) -> PathBuf {
    self.root.join("workspace.yaml")
}

pub fn artifact_path(&self, path: &LogicalPath) -> PathBuf {
    self.artifacts_dir().join(path.to_path_buf())
}

pub fn temp_path(&self, path: &LogicalPath) -> PathBuf {
    self.temp_dir().join(path.to_path_buf())
}

pub fn checkpoint_path(&self, path: &LogicalPath) -> PathBuf {
    self.checkpoints_dir().join(path.to_path_buf())
}
```

- [ ] **Step 4: Add `SessionConfig`**

Create `config.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionConfig {
    pub id: SessionId,
    pub root: PathBuf,
    pub cwd: LogicalPath,
}

impl SessionConfig {
    pub fn new(id: SessionId, root: impl Into<PathBuf>) -> Self {
        Self {
            id,
            root: root.into(),
            cwd: LogicalPath::root(),
        }
    }

    pub fn conventions(&self) -> PathConventions {
        PathConventions::for_session(&self.root, &self.id)
    }
}
```

- [ ] **Step 5: Add `LocalSessionResources`**

Create `resources.rs` with:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionResourceNamespace {
    Plan,
    Workspace,
    Artifacts,
    Temp,
    Checkpoints,
    Files,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionResourceMetadata {
    pub namespace: SessionResourceNamespace,
    pub path: Option<LogicalPath>,
    pub len: u64,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct LocalSessionResources {
    conventions: PathConventions,
}
```

Implement these command methods:

```rust
pub fn write_plan(&self, content: impl AsRef<str>) -> SessionResult<SessionResourceMetadata>;
pub fn write_workspace_yaml(&self, content: impl AsRef<str>) -> SessionResult<SessionResourceMetadata>;
pub fn write_artifact(&self, path: LogicalPath, bytes: &[u8]) -> SessionResult<SessionResourceMetadata>;
pub fn write_temp(&self, path: LogicalPath, bytes: &[u8]) -> SessionResult<SessionResourceMetadata>;
pub fn write_checkpoint(&self, path: LogicalPath, bytes: &[u8]) -> SessionResult<SessionResourceMetadata>;
pub fn write_file(&self, path: LogicalPath, bytes: &[u8]) -> SessionResult<SessionResourceMetadata>;
pub fn delete_artifact(&self, path: &LogicalPath) -> SessionResult<SessionResourceMetadata>;
pub fn delete_temp(&self, path: &LogicalPath) -> SessionResult<SessionResourceMetadata>;
pub fn delete_checkpoint(&self, path: &LogicalPath) -> SessionResult<SessionResourceMetadata>;
pub fn delete_file(&self, path: &LogicalPath) -> SessionResult<SessionResourceMetadata>;
```

Implement these query methods:

```rust
pub fn read_plan(&self) -> SessionResult<Vec<u8>>;
pub fn read_workspace_yaml(&self) -> SessionResult<Vec<u8>>;
pub fn read_artifact(&self, path: &LogicalPath) -> SessionResult<Vec<u8>>;
pub fn read_temp(&self, path: &LogicalPath) -> SessionResult<Vec<u8>>;
pub fn read_checkpoint(&self, path: &LogicalPath) -> SessionResult<Vec<u8>>;
pub fn read_file(&self, path: &LogicalPath) -> SessionResult<Vec<u8>>;
pub fn list_artifacts(&self) -> SessionResult<Vec<SessionResourceMetadata>>;
pub fn list_temp(&self) -> SessionResult<Vec<SessionResourceMetadata>>;
pub fn list_checkpoints(&self) -> SessionResult<Vec<SessionResourceMetadata>>;
pub fn list_files(&self) -> SessionResult<Vec<SessionResourceMetadata>>;
```

All `write_*` methods return `SessionResourceMetadata`. Runtime code converts metadata into `SessionResourceSnapshot` when it has `thread_id`, timestamp, and event context.

- [ ] **Step 5b: Add metadata JSON helpers**

In `metadata.rs`, add:

```rust
impl SessionMetadata {
    pub fn read_from_path(path: impl AsRef<Path>) -> SessionResult<Self> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|source| SessionError::io(path, source))?;
        serde_json::from_slice(&bytes).map_err(|source| SessionError::InvalidMetadata {
            path: path.to_path_buf(),
            message: source.to_string(),
        })
    }

    pub fn write_to_path(&self, path: impl AsRef<Path>) -> SessionResult<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| SessionError::io(parent, source))?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|source| SessionError::InvalidMetadata {
            path: path.to_path_buf(),
            message: source.to_string(),
        })?;
        std::fs::write(path, bytes).map_err(|source| SessionError::io(path, source))
    }
}
```

Add `SessionError::InvalidMetadata { path: PathBuf, message: String }`.

- [ ] **Step 6: Create directories on initialization**

`LocalSessionResources::with_conventions` must create:

```rust
for dir in [
    conventions.root(),
    conventions.files_dir().as_path(),
    conventions.artifacts_dir().as_path(),
    conventions.temp_dir().as_path(),
    conventions.checkpoints_dir().as_path(),
] {
    std::fs::create_dir_all(dir).map_err(|source| SessionError::io(dir, source))?;
}
```

- [ ] **Step 7: Export APIs**

Modify `session/mod.rs` and `prelude.rs` to export:

```rust
SessionConfig, LocalSessionResources, SessionResourceMetadata, SessionResourceNamespace
```

- [ ] **Step 8: Verify**

Run:

```bash
cargo test -p roci-core session::
```

Expected: all session tests pass.

---

## Task 3: Resource Event DTOs and Projection

**Files:**
- Modify: `crates/roci-core/src/agent/runtime/chat/domain.rs`
- Modify: `crates/roci-core/src/agent/runtime/chat/event.rs`
- Modify: `crates/roci-core/src/agent/runtime/chat/projector.rs`
- Test: `crates/roci-core/src/agent/runtime_tests/chat_contracts.rs`
- Test: `crates/roci-core/src/agent/runtime_tests/chat_projection.rs`

- [ ] **Step 1: Write event contract tests**

Add a test asserting serde names for new events:

```rust
#[test]
fn resource_event_payloads_have_stable_names() {
    let payload = AgentRuntimeEventPayload::ArtifactCreated {
        resource: test_session_resource_snapshot(SessionResourceNamespace::Artifacts, "out.txt"),
    };

    let encoded = serde_json::to_value(payload).unwrap();

    assert_eq!(encoded["type"], "artifact_created");
}
```

Add equivalents for `plan_written`, `workspace_updated`, `temp_file_written`, `checkpoint_created`, `session_file_written`, and `session_file_deleted`.

- [ ] **Step 2: Write projection test**

Add a projector test:

```rust
#[test]
fn projector_tracks_session_resource_snapshots() {
    let thread_id = ThreadId::new();
    let mut projector = ChatProjector::new(ChatRuntimeConfig {
        default_thread_id: Some(thread_id),
        ..Default::default()
    });
    let turn = projector.queue_turn(vec![ModelMessage::user("hi")]).turn_id;

    let event = projector.record_session_resource(
        turn,
        AgentRuntimeEventPayload::ArtifactCreated {
            resource: test_session_resource_snapshot(SessionResourceNamespace::Artifacts, "out.txt"),
        },
    ).unwrap();

    let snapshot = projector.read_thread(thread_id).unwrap();
    assert_eq!(event.seq, snapshot.last_seq);
    assert_eq!(snapshot.resources.artifacts.len(), 1);
}
```

- [ ] **Step 3: Run tests and confirm fail**

Run:

```bash
cargo test -p roci-core --features agent "agent::runtime::tests::chat_contracts"
cargo test -p roci-core --features agent "agent::runtime::tests::chat_projection"
```

Expected: compile failures for missing DTOs/events.

- [ ] **Step 4: Add snapshot DTOs**

In `domain.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionResourceSnapshot {
    pub namespace: SessionResourceNamespace,
    pub path: Option<LogicalPath>,
    pub len: u64,
    pub updated_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SessionResourcesSnapshot {
    pub plan: Option<SessionResourceSnapshot>,
    pub workspace: Option<SessionResourceSnapshot>,
    pub artifacts: Vec<SessionResourceSnapshot>,
    pub temp_files: Vec<SessionResourceSnapshot>,
    pub checkpoints: Vec<SessionResourceSnapshot>,
    pub files: Vec<SessionResourceSnapshot>,
}
```

Add `resources: SessionResourcesSnapshot` to `ThreadSnapshot`, initialize in `ThreadState::new`, `bootstrap_thread`, and import paths.

Add conversion helper in runtime/session integration code:

```rust
fn resource_snapshot_from_metadata(
    metadata: SessionResourceMetadata,
    metadata_json: serde_json::Value,
) -> SessionResourceSnapshot {
    SessionResourceSnapshot {
        namespace: metadata.namespace,
        path: metadata.path,
        len: metadata.len,
        updated_at: metadata.updated_at.unwrap_or_else(Utc::now),
        metadata: metadata_json,
    }
}
```

Do not make `LocalSessionResources` depend on chat DTOs. Session module returns `SessionResourceMetadata`; chat/runtime layer creates `SessionResourceSnapshot`.

- [ ] **Step 5: Add event payloads**

In `event.rs`:

```rust
PlanWritten { resource: SessionResourceSnapshot },
WorkspaceUpdated { resource: SessionResourceSnapshot },
ArtifactCreated { resource: SessionResourceSnapshot },
TempFileWritten { resource: SessionResourceSnapshot },
CheckpointCreated { resource: SessionResourceSnapshot },
SessionFileWritten { resource: SessionResourceSnapshot },
SessionFileDeleted { resource: SessionResourceSnapshot },
```

Use `#[serde(tag = "type", rename_all = "snake_case")]` already present.

- [ ] **Step 6: Project resource events**

Add a projector method:

```rust
pub fn record_session_resource(
    &mut self,
    turn_id: TurnId,
    payload: AgentRuntimeEventPayload,
) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
    let thread = self.thread_mut(turn_id.thread_id())?;
    thread.record_session_resource(turn_id, payload)
}
```

Inside `ThreadState`, update resource snapshot lists based on payload variant, then call existing `event(Some(turn_id), payload)`.

- [ ] **Step 7: Verify**

Run:

```bash
cargo test -p roci-core --features agent "agent::runtime::tests::chat_contracts"
cargo test -p roci-core --features agent "agent::runtime::tests::chat_projection"
```

Expected: tests pass.

---

## Task 4: AgentConfig Session Wiring and Plan Mirroring

**Files:**
- Modify: `crates/roci-core/src/agent/runtime/config.rs`
- Modify: `crates/roci-core/src/agent/runtime.rs`
- Modify: `crates/roci-core/src/agent/runtime/events.rs`
- Modify: `crates/roci-core/src/agent/runtime/run_loop.rs`
- Test: `crates/roci-core/src/agent/runtime_tests/session.rs`
- Modify: `crates/roci-core/src/agent/runtime_tests/mod.rs`

- [ ] **Step 1: Write runtime session tests**

Create `runtime_tests/session.rs`:

```rust
#[tokio::test]
async fn session_config_uses_jsonl_store_without_project_cwd_storage() {
    let temp = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let previous_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(project.path()).unwrap();
    let session_id = SessionId::parse("test-session").unwrap();
    let mut config = test_agent_config();
    config.session = Some(SessionConfig::new(session_id.clone(), temp.path()));

    let agent = AgentRuntime::try_new(test_registry(), RociConfig::default(), config).unwrap();
    agent.enqueue_turn(EnqueueTurnRequest {
        messages: vec![ModelMessage::user("hello")],
        generation_settings: None,
        approval_policy: None,
        collaboration_mode: None,
    }).await.unwrap();
    agent.wait_for_idle().await;
    std::env::set_current_dir(previous_cwd).unwrap();

    assert!(temp.path().join(session_id.as_str()).join("events.jsonl").exists());
    assert!(temp.path().join(session_id.as_str()).join("metadata.json").exists());
    assert!(!project.path().join(".roci").exists());
    assert!(!project.path().join(session_id.as_str()).exists());
}

#[tokio::test]
async fn plan_updates_are_mirrored_to_plan_md() {
    let temp = tempfile::tempdir().unwrap();
    let session_id = SessionId::parse("plan-session").unwrap();
    let mut config = test_agent_config();
    config.model = "plan-json:stub-model".parse().unwrap();
    config.session = Some(SessionConfig::new(session_id.clone(), temp.path()));

    let agent = AgentRuntime::try_new(
        registry_with_plan_json_provider("plan-json", "make a plan"),
        RociConfig::default(),
        config,
    ).unwrap();
    agent.enqueue_turn(EnqueueTurnRequest {
        messages: vec![ModelMessage::user("make a plan")],
        generation_settings: None,
        approval_policy: None,
        collaboration_mode: Some(CollaborationMode::Plan),
    }).await.unwrap();
    agent.wait_for_idle().await;

    let plan_path = temp.path().join(session_id.as_str()).join("plan.md");
    let plan = std::fs::read_to_string(plan_path).unwrap();
    assert!(plan.contains("make a plan"));
}

#[tokio::test]
async fn session_constructor_surfaces_corrupt_events_jsonl() {
    let temp = tempfile::tempdir().unwrap();
    let session_id = SessionId::parse("corrupt-session").unwrap();
    let session_root = temp.path().join(session_id.as_str());
    std::fs::create_dir_all(&session_root).unwrap();
    std::fs::write(session_root.join("events.jsonl"), b"not-json\n").unwrap();
    let mut config = test_agent_config();
    config.session = Some(SessionConfig::new(session_id, temp.path()));

    let err = AgentRuntime::try_new(test_registry(), RociConfig::default(), config).unwrap_err();

    assert!(err.to_string().contains("events.jsonl"));
    assert!(err.to_string().contains("line 1"));
}
```

Add this helper to `runtime_tests/support.rs`:

```rust
pub(super) fn registry_with_plan_json_provider(
    provider_key: &'static str,
    plan: &'static str,
) -> Arc<ProviderRegistry> {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(PlanJsonFactory { provider_key, plan }));
    Arc::new(registry)
}

struct PlanJsonFactory {
    provider_key: &'static str,
    plan: &'static str,
}

impl ProviderFactory for PlanJsonFactory {
    fn provider_keys(&self) -> &[&str] {
        std::slice::from_ref(&self.provider_key)
    }

    fn create(
        &self,
        _config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        Ok(Box::new(PlanJsonProvider {
            provider_key: self.provider_key.to_string(),
            model_id: model_id.to_string(),
            plan: self.plan.to_string(),
            capabilities: ModelCapabilities::default(),
        }))
    }
}

struct PlanJsonProvider {
    provider_key: String,
    model_id: String,
    plan: String,
    capabilities: ModelCapabilities,
}

#[async_trait]
impl ModelProvider for PlanJsonProvider {
    fn provider_name(&self) -> &str {
        &self.provider_key
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        Err(RociError::UnsupportedOperation(
            "stream-only plan-json test provider".to_string(),
        ))
    }

    async fn stream_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        let json = serde_json::json!({ "plan": self.plan }).to_string();
        let events = vec![
            Ok(TextStreamDelta {
                text: json,
                event_type: StreamEventType::TextDelta,
                tool_call: None,
                finish_reason: None,
                usage: None,
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            }),
            Ok(TextStreamDelta {
                text: String::new(),
                event_type: StreamEventType::Done,
                tool_call: None,
                finish_reason: None,
                usage: Some(crate::types::Usage::default()),
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            }),
        ];
        Ok(Box::pin(futures::stream::iter(events)))
    }
}
```

- [ ] **Step 2: Run tests and confirm fail**

Run:

```bash
cargo test -p roci-core --features agent "agent::runtime::tests::session"
```

Expected: compile failure because `AgentConfig.session` and mirroring do not exist.

- [ ] **Step 3: Add `AgentConfig.session`**

In `config.rs`:

```rust
use crate::session::SessionConfig;
use crate::tools::SandboxProvider;

pub struct AgentConfig {
    pub session: Option<SessionConfig>,
    pub sandbox_provider: Option<Arc<dyn SandboxProvider>>,
}
```

Add the fields to the existing `AgentConfig` struct; keep every existing field unchanged.

Set `session: None` and `sandbox_provider: None` in `Default`.

- [ ] **Step 4: Add fallible runtime constructor for sessions**

Keep existing `AgentRuntime::new(registry, roci_config, config) -> Self` available for non-session callers. Add:

```rust
pub fn try_new(
    registry: Arc<ProviderRegistry>,
    roci_config: RociConfig,
    config: AgentConfig,
) -> Result<Self, RociError> {
    Self::new_inner(registry, roci_config, config)
}
```

Move current constructor body into `new_inner(registry, roci_config, config) -> Result<Self, RociError>`. Implement `new(registry, roci_config, config)` as:

```rust
pub fn new(
    registry: Arc<ProviderRegistry>,
    roci_config: RociConfig,
    config: AgentConfig,
) -> Self {
    Self::new_inner(registry, roci_config, config)
        .expect("AgentRuntime::new failed; use AgentRuntime::try_new for fallible session setup")
}
```

This preserves existing call sites while giving session callers a non-panicking path.

- [ ] **Step 5: Wire session store/resources in constructor**

When `config.session` exists:

```rust
let session_conventions = config.session.as_ref().map(SessionConfig::conventions);
let session_resources = session_conventions
    .as_ref()
    .map(LocalSessionResources::with_conventions)
    .transpose()
    .map_err(|err| RociError::InvalidState(err.to_string()))?;
let session_fs = session_conventions
    .as_ref()
    .map(LocalSessionFs::with_conventions)
    .transpose()
    .map_err(|err| RociError::InvalidState(err.to_string()))?;
if let (Some(session), Some(conventions)) = (&config.session, &session_conventions) {
    let metadata = SessionMetadata::new(
        session.id.clone(),
        std::env::current_dir().ok(),
        None,
    );
    metadata
        .write_to_path(conventions.metadata_file())
        .map_err(|err| RociError::InvalidState(err.to_string()))?;
}
let runtime_event_store = if let Some(conventions) = session_conventions.as_ref() {
    Arc::new(JsonlAgentRuntimeEventStore::open(conventions.events_file())
        .map_err(Self::map_chat_projection_error)?)
} else {
    config.chat.event_store.clone().unwrap_or_else(|| {
        Arc::new(InMemoryAgentRuntimeEventStore::with_replay_capacity(replay_capacity))
    })
};
```

- [ ] **Step 6: Store resources on runtime**

Add fields:

```rust
session_config: Option<SessionConfig>,
session_fs: Option<Arc<LocalSessionFs>>,
session_resources: Option<Arc<LocalSessionResources>>,
sandbox_provider: Option<Arc<dyn SandboxProvider>>,
```

Keep them cloneable and read-only after construction for this slice.

- [ ] **Step 7: Mirror intercepted `PlanUpdated`**

In `events.rs`, after the existing `projector.update_plan` call succeeds, if `session_resources` exists:

```rust
let resource = session_resources.write_plan(plan)?;
let resource = resource_snapshot_from_metadata(resource, serde_json::Value::Null);
events.push(projector.record_session_resource(
    turn_id,
    AgentRuntimeEventPayload::PlanWritten { resource },
)?);
```

Do not write `plan.md` if projection fails. Do not make `plan.md` source of truth.

- [ ] **Step 8: Mirror structured plan mode**

In `run_loop.rs::project_structured_plan`, after the existing `projector.update_plan` call succeeds, mirror the same plan to `plan.md` and publish both events in order.

- [ ] **Step 9: Add runtime resource publish APIs**

Add public async methods on `AgentRuntime`:

```rust
pub async fn write_workspace_yaml(&self, workspace: impl AsRef<str>) -> Result<SessionResourceSnapshot, RociError>;
pub async fn write_artifact(&self, path: LogicalPath, bytes: &[u8]) -> Result<SessionResourceSnapshot, RociError>;
pub async fn write_temp_file(&self, path: LogicalPath, bytes: &[u8]) -> Result<SessionResourceSnapshot, RociError>;
pub async fn write_checkpoint(&self, path: LogicalPath, bytes: &[u8]) -> Result<SessionResourceSnapshot, RociError>;
pub async fn write_session_file(&self, path: LogicalPath, bytes: &[u8]) -> Result<SessionResourceSnapshot, RociError>;
pub async fn delete_session_file(&self, path: LogicalPath) -> Result<SessionResourceSnapshot, RociError>;
```

Each method:

1. Requires configured session resources/fs.
2. Writes/deletes the resource.
3. Converts `SessionResourceMetadata` into `SessionResourceSnapshot`.
4. Records and publishes matching explicit event:
   - `WorkspaceUpdated`
   - `ArtifactCreated`
   - `TempFileWritten`
   - `CheckpointCreated`
   - `SessionFileWritten`
   - `SessionFileDeleted`

Use `default_thread_id()` when no active turn exists. Add one integration test per runtime method. Each test calls the method, asserts file state on disk, reads the runtime snapshot, and asserts the matching explicit event payload was published.

- [ ] **Step 10: Verify**

Run:

```bash
cargo test -p roci-core --features agent "agent::runtime::tests::session"
cargo test -p roci-core --features agent "agent::runtime::tests::chat_runtime"
```

Expected: session tests and existing chat runtime tests pass.

---

## Task 5: ToolExecutionContext Session Fields

**Files:**
- Modify: `crates/roci-core/src/tools/tool.rs`
- Modify: `crates/roci-core/src/agent_loop/runner.rs`
- Modify: `crates/roci-core/src/agent_loop/runner/tooling.rs`
- Modify: `crates/roci-core/src/agent/runtime/run_loop.rs`
- Test: `crates/roci-core/src/agent_loop/runner/tests/tool_execution.rs`

- [ ] **Step 1: Write context-threading test**

Add a runner-level test with a custom tool:

```rust
#[derive(Default)]
struct RecordingSandboxProvider {
    seen: AtomicBool,
}

impl RecordingSandboxProvider {
    fn mark_seen(&self) {
        self.seen.store(true, Ordering::SeqCst);
    }

    fn was_seen(&self) -> bool {
        self.seen.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SandboxProvider for RecordingSandboxProvider {
    async fn validate_shell_command(
        &self,
        _command: &str,
        _cwd: &LogicalPath,
    ) -> Result<(), RociError> {
        self.mark_seen();
        Ok(())
    }
}

#[tokio::test]
async fn run_request_threads_session_context_to_tools() {
    let (runner, _requests) = test_runner(ProviderScenario::ToolCallWithUsageThenTextWithUsage);
    let temp = tempfile::tempdir().unwrap();
    let session_fs = Arc::new(LocalSessionFs::new(temp.path().join("session")).unwrap());
    let seen_cwd = Arc::new(std::sync::Mutex::new(None));
    let seen_cwd_for_tool = seen_cwd.clone();
    let tool = Arc::new(AgentTool::new(
        "noop_tool",
        "capture session context",
        AgentToolParameters::empty(),
        move |_args, ctx: ToolExecutionContext| {
            let seen_cwd_for_tool = seen_cwd_for_tool.clone();
            async move {
                *seen_cwd_for_tool.lock().unwrap() = ctx.session_cwd.clone();
                Ok(serde_json::json!({ "has_session": ctx.session_fs.is_some() }))
            }
        },
    ));

    let request = RunRequest::new(test_model(), vec![ModelMessage::user("run tool")])
        .with_tools(vec![tool])
        .with_session_context(session_fs, LogicalPath::parse("work").unwrap());

    let handle = runner.start(request).await.unwrap();
    let result = timeout(Duration::from_secs(2), handle.wait()).await.unwrap();

    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(seen_cwd.lock().unwrap().as_ref().unwrap().as_str(), "work");
}

#[tokio::test]
async fn run_request_threads_sandbox_provider_to_tools() {
    let (runner, _requests) = test_runner(ProviderScenario::ToolCallWithUsageThenTextWithUsage);
    let temp = tempfile::tempdir().unwrap();
    let session_fs = Arc::new(LocalSessionFs::new(temp.path().join("session")).unwrap());
    let provider = Arc::new(RecordingSandboxProvider::default());
    let seen_provider = provider.clone();
    let tool = Arc::new(AgentTool::new(
        "noop_tool",
        "capture sandbox provider",
        AgentToolParameters::empty(),
        move |_args, ctx: ToolExecutionContext| {
            let seen_provider = seen_provider.clone();
            async move {
                assert!(ctx.sandbox_provider.is_some());
                seen_provider.mark_seen();
                Ok(serde_json::json!({ "ok": true }))
            }
        },
    ));

    let request = RunRequest::new(test_model(), vec![ModelMessage::user("run tool")])
        .with_tools(vec![tool])
        .with_session_context(session_fs, LogicalPath::root())
        .with_sandbox_provider(provider.clone());

    let handle = runner.start(request).await.unwrap();
    let result = timeout(Duration::from_secs(2), handle.wait()).await.unwrap();

    assert_eq!(result.status, RunStatus::Completed);
    assert!(provider.was_seen());
}
```

Use `test_runner(ProviderScenario::ToolCallWithUsageThenTextWithUsage)` and name the capture tool `noop_tool`, because that scenario calls `noop_tool` on its first provider response.

- [ ] **Step 2: Run test and confirm fail**

Run:

```bash
cargo test -p roci-core --features agent "agent_loop::runner::tests::tool_execution"
```

Expected: compile failure because context fields do not exist.

- [ ] **Step 3: Add session fields to context**

In `tools/tool.rs`:

```rust
pub struct ToolExecutionContext {
    pub session_fs: Option<Arc<dyn SessionFs + Send + Sync>>,
    pub session_cwd: Option<LogicalPath>,
    pub sandbox_provider: Option<Arc<dyn SandboxProvider>>,
}
```

Add the fields to the existing `ToolExecutionContext` struct; keep every existing field unchanged.

- [ ] **Step 4: Add sandbox seam trait**

Add a minimal trait in `tools/tool.rs` or a new `tools/sandbox.rs`:

```rust
#[async_trait]
pub trait SandboxProvider: Send + Sync {
    async fn validate_shell_command(
        &self,
        command: &str,
        cwd: &LogicalPath,
    ) -> Result<(), RociError>;
}
```

Default runtime can pass `None`; roci-tools shell will use local classifier if `session_fs` is present and no provider exists.

- [ ] **Step 5: Thread fields through `RunRequest`**

In `runner.rs` add:

```rust
pub session_fs: Option<Arc<dyn SessionFs + Send + Sync>>,
pub session_cwd: Option<LogicalPath>,
pub sandbox_provider: Option<Arc<dyn SandboxProvider>>,
```

Add builder:

```rust
pub fn with_session_context(
    mut self,
    session_fs: Arc<dyn SessionFs + Send + Sync>,
    session_cwd: LogicalPath,
) -> Self {
    self.session_fs = Some(session_fs);
    self.session_cwd = Some(session_cwd);
    self
}

pub fn with_sandbox_provider(mut self, provider: Arc<dyn SandboxProvider>) -> Self {
    self.sandbox_provider = Some(provider);
    self
}
```

- [ ] **Step 6: Populate context in `tooling.rs`**

At context construction:

```rust
let ctx = ToolExecutionContext {
    metadata: serde_json::Value::Null,
    tool_call_id: Some(call.id.clone()),
    tool_name: Some(call.name.clone()),
    session_fs: request.session_fs.clone(),
    session_cwd: request.session_cwd.clone(),
    sandbox_provider: request.sandbox_provider.clone(),
    #[cfg(feature = "agent")]
    request_user_input: user_input_callback.cloned(),
};
```

Update `run_tool_phase` to carry `request.session_fs.clone()`, `request.session_cwd.clone()`, and `request.sandbox_provider.clone()` into `execute_tool_call`, then set those values on `ToolExecutionContext` at the construction site above.

- [ ] **Step 7: Runtime passes session fields**

In `AgentRuntime::run_loop`, when building `RunRequest`, call `with_session_context` if `self.session_config` and `self.session_fs` exist. Also call `with_sandbox_provider` when `self.sandbox_provider` exists.

- [ ] **Step 8: Verify**

Run:

```bash
cargo test -p roci-core --features agent "agent_loop::runner::tests::tool_execution"
```

Expected: context-threading test and existing tool execution tests pass.

---

## Task 6: Session-Aware Built-In File Tools

**Files:**
- Modify: `crates/roci-tools/src/builtin/common.rs`
- Modify: `crates/roci-tools/src/builtin/read_file.rs`
- Modify: `crates/roci-tools/src/builtin/write_file.rs`
- Modify: `crates/roci-tools/src/builtin/list_directory.rs`
- Modify: `crates/roci-tools/src/builtin/grep.rs`
- Test: `crates/roci-tools/src/builtin/tests.rs`

- [ ] **Step 1: Add session tool tests**

Add tests:

```rust
fn session_ctx(root: &Path) -> ToolExecutionContext {
    ToolExecutionContext {
        session_fs: Some(Arc::new(LocalSessionFs::new(root.join("session")).unwrap())),
        session_cwd: Some(LogicalPath::parse("work").unwrap()),
        ..ToolExecutionContext::default()
    }
}

#[tokio::test]
async fn read_file_uses_session_cwd_when_present() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = session_ctx(temp.path());
    let fs = ctx.session_fs.as_ref().unwrap();
    fs.write(&LogicalPath::parse("work/hello.txt").unwrap(), b"hello").unwrap();

    let result = read_file_tool()
        .execute(&args(serde_json::json!({"path": "hello.txt"})), &ctx)
        .await
        .unwrap();

    assert_eq!(result["content"], "hello");
}

#[tokio::test]
async fn write_file_uses_session_cwd_when_present() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = session_ctx(temp.path());

    write_file_tool()
        .execute(&args(serde_json::json!({"path": "out.txt", "content": "ok"})), &ctx)
        .await
        .unwrap();

    let fs = ctx.session_fs.as_ref().unwrap();
    assert_eq!(
        fs.read(&LogicalPath::parse("work/out.txt").unwrap()).unwrap(),
        b"ok"
    );
}

#[tokio::test]
async fn session_file_tools_reject_absolute_and_parent_paths() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = session_ctx(temp.path());

    assert!(read_file_tool()
        .execute(&args(serde_json::json!({"path": "/etc/passwd"})), &ctx)
        .await
        .is_err());
    assert!(write_file_tool()
        .execute(&args(serde_json::json!({"path": "../out.txt", "content": "x"})), &ctx)
        .await
        .is_err());
}
```

Add symlink escape test under `#[cfg(unix)]`.
Add grep-specific symlink escape test:

```rust
#[cfg(unix)]
#[tokio::test]
async fn session_grep_does_not_follow_symlink_escape() {
    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside.txt");
    std::fs::write(&outside, "secret needle").unwrap();
    let ctx = session_ctx(temp.path());
    let session_root = ctx.session_fs.as_ref().unwrap().files_root().to_path_buf();
    std::fs::create_dir_all(session_root.join("work")).unwrap();
    std::os::unix::fs::symlink(&outside, session_root.join("work/escape.txt")).unwrap();

    let result = grep_tool()
        .execute(&args(serde_json::json!({"pattern": "needle", "path": "."})), &ctx)
        .await
        .unwrap();

    assert_eq!(result["exit_code"], 1);
    assert!(!result["output"].as_str().unwrap().contains("secret"));
}
```

Add non-session regression tests proving host absolute path behavior still works for `read_file` and `grep`.

- [ ] **Step 2: Run tests and confirm fail**

Run:

```bash
cargo test -p roci-tools
```

Expected: compile failures until context fields and helper functions exist.

- [ ] **Step 3: Add session path helper**

In `common.rs`:

```rust
pub(super) fn resolve_session_path(
    ctx: &ToolExecutionContext,
    raw_path: &str,
) -> Result<Option<LogicalPath>, RociError> {
    let Some(cwd) = ctx.session_cwd.as_ref() else {
        return Ok(None);
    };
    let path = cwd.join(raw_path).map_err(|err| RociError::ToolExecution {
        tool_name: ctx.tool_name.clone().unwrap_or_else(|| "tool".to_string()),
        message: err.to_string(),
    })?;
    Ok(Some(path))
}
```

- [ ] **Step 4: Update read/write/list**

For each file tool:

```rust
if let (Some(session_fs), Some(path)) = (&ctx.session_fs, resolve_session_path(&ctx, path)?) {
    let bytes = session_fs.read(&path).map_err(|err| RociError::ToolExecution {
        tool_name: "read_file".into(),
        message: err.to_string(),
    })?;
    let content = String::from_utf8(bytes).map_err(|err| RociError::ToolExecution {
        tool_name: "read_file".into(),
        message: err.to_string(),
    })?;
    let total_bytes = content.len();
    let truncated = total_bytes > READ_FILE_MAX_BYTES;
    let display = if truncated {
        let mut s = truncate_utf8(&content, READ_FILE_MAX_BYTES);
        s.push_str("\n... (truncated)");
        s
    } else {
        content
    };
    return Ok(serde_json::json!({
        "content": display,
        "bytes": total_bytes,
        "truncated": truncated,
    }));
}
```

Fallback to existing host path behavior when `session_fs` or `session_cwd` is missing.

- [ ] **Step 5: Update grep**

If session context exists:

- Resolve `path` argument or default `"."` through `session_cwd`.
- Walk directories through `SessionFs::list` and read file bytes through `SessionFs::read`.
- Do not spawn host `grep` for session context.
- Skip symlink entries or return a tool error for symlink entries; do not follow them.
- Match lines in UTF-8 files and format output like `path:line:content`.
- Return exit code `0` when matches exist, `1` when none exist.
- Return JSON with `exit_code`, `output`, and `truncated` fields, matching current host-mode grep.

Do not allow grep to search outside session root.

- [ ] **Step 6: Verify**

Run:

```bash
cargo test -p roci-tools
```

Expected: all roci-tools tests pass.

---

## Task 7: Session Shell Classifier and Sandbox Seam

**Files:**
- Modify: `crates/roci-tools/src/builtin/common.rs`
- Modify: `crates/roci-tools/src/builtin/shell.rs`
- Test: `crates/roci-tools/src/builtin/tests.rs`

- [ ] **Step 1: Add shell session tests**

Add tests:

```rust
#[tokio::test]
async fn session_shell_runs_in_session_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = session_ctx(temp.path());

    let result = shell_tool()
        .execute(&args(serde_json::json!({"command": "pwd"})), &ctx)
        .await
        .unwrap();

    assert!(result["output"].as_str().unwrap().contains("/files/work"));
}

#[tokio::test]
async fn session_shell_rejects_obvious_escape_commands() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = session_ctx(temp.path());

    for command in [
        "/bin/cat file.txt",
        "cat /etc/passwd",
        "cat ../secret",
        "cd /",
        "echo hi > /tmp/out",
        "echo hi >/tmp/out",
        "echo hi 2>/tmp/err",
        "cat </etc/passwd",
    ] {
        let err = shell_tool()
            .execute(&args(serde_json::json!({"command": command})), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("session shell command denied"));
    }
}
```

- [ ] **Step 2: Run tests and confirm fail**

Run:

```bash
cargo test -p roci-tools session_shell
```

Expected: tests fail because classifier/current_dir not implemented.

- [ ] **Step 3: Add classifier**

In `common.rs`:

```rust
pub(super) fn validate_session_shell_command(command: &str) -> Result<(), String> {
    let trimmed = command.trim_start();
    if trimmed.starts_with('/') {
        return Err("command starts with absolute path".to_string());
    }
    let denied_substrings = [
        " /", "\t/", "../", " cd /", "cd /", "> /", ">/", ">> /", ">>/",
        "2> /", "2>/", "< /", "</", " --output=/", "rm -rf", "sudo ",
        "chmod ", "chown ",
    ];
    if let Some(pattern) = denied_substrings.iter().find(|pattern| command.contains(*pattern)) {
        return Err(format!("matched denied pattern `{pattern}`"));
    }
    Ok(())
}
```

Keep this conservative. It is not a security boundary.

- [ ] **Step 4: Run shell in session cwd**

In `shell.rs`, if session context exists:

```rust
validate_session_shell_command(command).map_err(|reason| RociError::ToolExecution {
    tool_name: "shell".into(),
    message: format!("session shell command denied: {reason}"),
})?;

let cwd = ctx
    .session_fs
    .as_ref()
    .unwrap()
    .files_root()
    .join(ctx.session_cwd.as_ref().unwrap().to_path_buf());

tokio::process::Command::new("sh")
    .arg("-c")
    .arg(command)
    .current_dir(cwd)
    .output()
```

Use sandbox provider first when present, then still run local classifier as a conservative default:

```rust
if let Some(provider) = &ctx.sandbox_provider {
    provider.validate_shell_command(command, ctx.session_cwd.as_ref().unwrap()).await?;
}
validate_session_shell_command(command)?;
```

- [ ] **Step 5: Verify**

Run:

```bash
cargo test -p roci-tools session_shell
cargo test -p roci-tools
```

Expected: shell tests and full roci-tools suite pass.

---

## Task 8: Integration Docs

**Files:**
- Modify: `docs/agent-runtime-chat.md`
- Modify: `docs/ARCHITECTURE.md`

- [ ] **Step 1: Update chat runtime persistence docs**

In `docs/agent-runtime-chat.md`, add:

```markdown
### Durable session event store

When `AgentConfig.session` is configured, Roci uses a strict JSONL
`JsonlAgentRuntimeEventStore` at `<session_root>/<session_id>/events.jsonl`.
Malformed nonblank committed lines fail replay with a visible error. Hosts that
need salvage behavior should use a separate tolerant history/repair layer, not
runtime event replay.
```

- [ ] **Step 2: Document session root boundary**

In `docs/ARCHITECTURE.md`, under `roci-core`, add:

```markdown
Durable sessions are opt-in and host-rooted. Roci does not write session data
into project cwd unless the host explicitly chooses that directory as the
session root. `host_cwd` is metadata/import context; tool filesystem operations
use logical paths under `files/`.
```

- [ ] **Step 3: Verify docs wording**

Run:

```bash
rg -n "project cwd|events.jsonl|JsonlAgentRuntimeEventStore|tolerant" docs/agent-runtime-chat.md docs/ARCHITECTURE.md
```

Expected: docs state strict runtime replay and host-supplied session root.

---

## Task 9: Full Verification

**Files:**
- No edits unless fixing failures in owned scope.

- [ ] **Step 1: Run focused tests**

Run:

```bash
cargo test -p roci-core session::
cargo test -p roci-core --features agent "agent::runtime::tests::session"
cargo test -p roci-core --features agent "agent::runtime::tests::chat_contracts"
cargo test -p roci-core --features agent "agent::runtime::tests::chat_projection"
cargo test -p roci-core --features agent "agent_loop::runner::tests::tool_execution"
cargo test -p roci-tools
```

Expected: all pass.

- [ ] **Step 2: Run formatting and lint**

Run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features full -- -D warnings
```

Expected: both pass. If `full` feature is not defined in this workspace, run:

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Record which command was used.

- [ ] **Step 3: Run full tests**

Run:

```bash
cargo test
```

Expected: full workspace tests pass.

- [ ] **Step 4: Run live tmux provider smoke**

Because tool/runtime loop paths changed, run live provider smoke per `docs/testing.md`.

Start tmux:

```bash
tmux new-session -d -s roci-live-provider \
  'cd /Users/adityasharma/Projects/roci && \
   LMSTUDIO_BASE_URL=http://127.0.0.1:1234 \
   cargo run -q -p roci-cli --features roci/lmstudio -- \
   chat --no-skills --model "lmstudio:<model-id>" \
   --temperature 0 --max-tokens 32 \
   "Reply exactly: roci-local-smoke-ok"; \
   status=$?; printf "\n[roci smoke exit=%s]\n" "$status"; exec zsh'
echo "attach: tmux attach -t roci-live-provider"
```

If local model unavailable, report that and use configured provider relevant to changed tool/runtime behavior.

- [ ] **Step 5: Final evidence**

Report:

- changed files
- test commands and outcomes
- live tmux attach command and result
- any skipped provider verification with reason
