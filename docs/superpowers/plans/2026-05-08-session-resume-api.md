# Session Resume API Implementation Plan

> **For agentic workers:** Prefer `subagent-driven-development` for execution when available. Task implementers own task work and review fixes; integration owner owns final integration. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add async durable session create/open/import/export and resume APIs for `tsq-r0c1ses7.3`.

**Architecture:** `LocalSessionStore` owns all session filesystem lifecycle and strict replay. `events.jsonl` remains canonical semantic runtime state with `runtime.snapshot.json` as cache, while `model_messages.jsonl` is the canonical provider ledger with `model_messages.snapshot.json` as cache. `AgentRuntime::{new,try_new}` consume prepared state only; `AgentRuntime::resume_session` seeds projector and provider ledger from `SessionResumeState`.

**Tech Stack:** Rust 2021, `tokio`, `serde`, `serde_json`, `chrono`, current `roci-core` session/runtime/chat modules, existing `JsonlAgentRuntimeEventStore`, existing fake-provider runtime tests.

---

## File Structure

- Create `crates/roci-core/src/session/ledger.rs`
  - Append-only `model_messages.jsonl` record format, strict replay, compaction records, snapshot cache helpers.
- Create `crates/roci-core/src/session/snapshot.rs`
  - `CreateSessionOptions`, `ImportPolicy`, `SessionSnapshot`, `SessionResumeState`, resource manifest DTOs, runtime/provider cache DTOs.
- Create `crates/roci-core/src/session/store.rs`
  - `LocalSessionStore` async create/open/export/import, single-writer guard, metadata preservation, snapshot cache handling.
- Modify `crates/roci-core/src/session/metadata.rs`
  - Add `last_activity_at` with serde default migration.
- Modify `crates/roci-core/src/session/error.rs`
  - Add conflict/lock/runtime projection/ledger corruption variants.
- Modify `crates/roci-core/src/session/path.rs`
  - Add `runtime_snapshot_file()`, `provider_ledger_file()`, `provider_ledger_snapshot_file()`.
- Modify `crates/roci-core/src/session/mod.rs`
  - Export store/snapshot/ledger public APIs.
- Modify `crates/roci-core/src/prelude.rs`
  - Re-export stable session store/snapshot APIs.
- Modify `crates/roci-core/src/agent/runtime/chat/projector.rs`
  - Add event replay into projector and normalized-open helper support.
- Modify `crates/roci-core/src/agent/runtime/chat/mod.rs`
  - Export `ChatProjector` replay helpers used by session store tests.
- Modify `crates/roci-core/src/agent/runtime.rs`
  - Remove session file creation from constructor, accept prepared session handles/state, add `resume_session`.
- Modify `crates/roci-cli/src/chat.rs`
  - Keep current CLI chat/session flags compiling by creating/opening sessions through `LocalSessionStore` before runtime construction.
- Modify `crates/roci-core/src/agent/runtime/config.rs`
  - Keep `AgentConfig.session` as host-visible session identity; do not add lifecycle IO knobs.
- Modify `crates/roci-core/src/agent/runtime/lifecycle.rs`
  - Make `import_thread`, `reset`, and resume flow update provider ledger.
- Modify `crates/roci-core/src/agent/runtime/mutations.rs`
  - Make `replace_messages` update provider ledger.
- Modify `crates/roci-core/src/agent/runtime/run_loop.rs`
  - Append provider ledger messages on committed provider results.
- Modify `crates/roci-core/src/agent/runtime_tests/session.rs`
  - Add store, ledger, resume, normalization, metadata, and import/export tests.
- Modify `docs/agent-runtime-chat.md`
  - Document resume/store split and provider ledger.
- Modify `docs/ARCHITECTURE.md`
  - Document `LocalSessionStore` responsibility boundary.

---

## Task 0: Baseline Guard

**Files:**
- Read only.

- [ ] **Step 1: Confirm worktree**

Run:

```bash
git status --short
```

Expected: only planning/spec docs may be dirty. Do not revert unrelated changes.

- [ ] **Step 2: Run narrow baseline**

Run:

```bash
cargo test -p roci-core session::
cargo test -p roci-core --features agent "agent::runtime::tests::session"
```

Expected: pass. If either fails, stop and diagnose before implementing `.3`.

---

## Task 1: Metadata, Paths, And Error Surface

**Files:**
- Modify: `crates/roci-core/src/session/metadata.rs`
- Modify: `crates/roci-core/src/session/error.rs`
- Modify: `crates/roci-core/src/session/path.rs`
- Test: `crates/roci-core/src/session/metadata.rs`
- Test: `crates/roci-core/src/session/path.rs`

- [ ] **Step 1: Add failing metadata migration test**

Add to `metadata.rs` tests:

```rust
#[test]
fn session_metadata_defaults_last_activity_to_updated_at() {
    let updated_at = "2026-05-08T03:00:00Z";
    let json = format!(
        r#"{{
          "id":"session-old",
          "title":null,
          "created_at":"2026-05-08T02:00:00Z",
          "updated_at":"{updated_at}",
          "host_cwd":null,
          "import_source":null
        }}"#
    );

    let metadata: SessionMetadata =
        serde_json::from_str(&json).expect("old metadata should deserialize");

    assert_eq!(metadata.last_activity_at, metadata.updated_at);
}
```

- [ ] **Step 2: Add failing path convention test**

Add to `path.rs` tests:

```rust
#[test]
fn path_conventions_include_resume_cache_and_provider_ledger_paths() {
    let id = SessionId::parse("session-layout").unwrap();
    let conventions = PathConventions::for_session("/tmp/roci-sessions", &id);

    assert_eq!(
        conventions.runtime_snapshot_file(),
        std::path::PathBuf::from("/tmp/roci-sessions/session-layout/runtime.snapshot.json")
    );
    assert_eq!(
        conventions.provider_ledger_file(),
        std::path::PathBuf::from("/tmp/roci-sessions/session-layout/model_messages.jsonl")
    );
    assert_eq!(
        conventions.provider_ledger_snapshot_file(),
        std::path::PathBuf::from(
            "/tmp/roci-sessions/session-layout/model_messages.snapshot.json"
        )
    );
}
```

- [ ] **Step 3: Run tests and confirm fail**

Run:

```bash
cargo test -p roci-core session::metadata::tests::session_metadata_defaults_last_activity_to_updated_at
cargo test -p roci-core session::path::tests::path_conventions_include_resume_cache_and_provider_ledger_paths
```

Expected: compile failures for missing `last_activity_at` and missing path methods.

- [ ] **Step 4: Implement metadata field with default**

In `metadata.rs`, add a private serde helper and expose a concrete field:

```rust
#[derive(Deserialize)]
struct SessionMetadataWire {
    id: SessionId,
    title: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    last_activity_at: Option<DateTime<Utc>>,
    host_cwd: Option<PathBuf>,
    import_source: Option<PathBuf>,
}

impl<'de> Deserialize<'de> for SessionMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = SessionMetadataWire::deserialize(deserializer)?;
        Ok(Self {
            id: wire.id,
            title: wire.title,
            created_at: wire.created_at,
            updated_at: wire.updated_at,
            last_activity_at: wire.last_activity_at.unwrap_or(wire.updated_at),
            host_cwd: wire.host_cwd,
            import_source: wire.import_source,
        })
    }
}
```

Update `SessionMetadata`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionMetadata {
    pub id: SessionId,
    pub title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub host_cwd: Option<PathBuf>,
    pub import_source: Option<PathBuf>,
}
```

`SessionMetadata::new` must set:

```rust
last_activity_at: now,
```

- [ ] **Step 5: Implement path methods**

In `path.rs`, add:

```rust
pub fn runtime_snapshot_file(&self) -> PathBuf {
    self.root.join("runtime.snapshot.json")
}

pub fn provider_ledger_file(&self) -> PathBuf {
    self.root.join("model_messages.jsonl")
}

pub fn provider_ledger_snapshot_file(&self) -> PathBuf {
    self.root.join("model_messages.snapshot.json")
}
```

- [ ] **Step 6: Add error variants**

In `error.rs`, add:

```rust
#[error("session already exists: {path}")]
AlreadyExists { path: PathBuf },
#[error("session is already open for writing: {path}")]
AlreadyOpen { path: PathBuf },
#[error("runtime projection error for {path}: {message}")]
RuntimeProjection { path: PathBuf, message: String },
#[error("invalid provider ledger at {path}: {message}")]
InvalidProviderLedger { path: PathBuf, message: String },
```

- [ ] **Step 7: Verify task**

Run:

```bash
cargo test -p roci-core session::metadata::tests::session_metadata_defaults_last_activity_to_updated_at
cargo test -p roci-core session::path::tests::path_conventions_include_resume_cache_and_provider_ledger_paths
cargo fmt --all -- --check
```

Expected: all pass.

---

## Task 2: Provider Ledger Store

**Files:**
- Create: `crates/roci-core/src/session/ledger.rs`
- Modify: `crates/roci-core/src/session/mod.rs`
- Test: `crates/roci-core/src/session/ledger.rs`

- [ ] **Step 1: Add ledger module export**

In `session/mod.rs`, add:

```rust
mod ledger;

pub use ledger::{
    LocalProviderLedger, ProviderLedgerRecord, ProviderLedgerSnapshot,
    ProviderLedgerState,
};
```

- [ ] **Step 2: Write failing append/replay test**

Create `ledger.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::runtime::chat::ThreadId;
    use crate::types::ModelMessage;
    use tempfile::tempdir;

    #[tokio::test]
    async fn provider_ledger_appends_and_replays_messages() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("model_messages.jsonl");
        let ledger = LocalProviderLedger::open(&path).await.unwrap();
        let thread_id = ThreadId::new();
        let user = ModelMessage::user("hello");
        let assistant = ModelMessage::assistant("hi");

        ledger
            .append_message(thread_id, user.clone())
            .await
            .unwrap();
        ledger
            .append_message(thread_id, assistant.clone())
            .await
            .unwrap();
        drop(ledger);

        let reopened = LocalProviderLedger::open(&path).await.unwrap();
        let state = reopened.state_for_thread(thread_id).await.unwrap();

        assert_eq!(state.latest_seq, 2);
        assert_eq!(state.messages, vec![user, assistant]);
    }
}
```

- [ ] **Step 3: Write failing compaction test**

Add:

```rust
#[tokio::test]
async fn provider_ledger_replays_latest_compacted_history_plus_suffix() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("model_messages.jsonl");
    let ledger = LocalProviderLedger::open(&path).await.unwrap();
    let thread_id = ThreadId::new();
    let old = ModelMessage::user("old");
    let summary = ModelMessage::system("summary");
    let new = ModelMessage::user("new");

    ledger
        .append_message(thread_id, old)
        .await
        .unwrap();
    ledger
        .append_compacted(
            thread_id,
            1,
            vec![summary.clone()],
        )
        .await
        .unwrap();
    ledger
        .append_message(thread_id, new.clone())
        .await
        .unwrap();
    drop(ledger);

    let reopened = LocalProviderLedger::open(&path).await.unwrap();
    let state = reopened.state_for_thread(thread_id).await.unwrap();

    assert_eq!(state.messages, vec![summary, new]);
    assert_eq!(state.latest_seq, 3);
}
```

- [ ] **Step 4: Write failing strict corruption tests**

Add:

```rust
#[tokio::test]
async fn provider_ledger_rejects_corrupt_committed_line_with_path_and_line() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("model_messages.jsonl");
    tokio::fs::write(&path, b"not-json\n").await.unwrap();

    let err = LocalProviderLedger::open(&path).await.unwrap_err();
    let message = err.to_string();

    assert!(message.contains("model_messages.jsonl"));
    assert!(message.contains("line 1"));
}

#[tokio::test]
async fn provider_ledger_rejects_final_nonblank_line_without_newline() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("model_messages.jsonl");
    tokio::fs::write(&path, br#"{"type":"ledger_invalidated"}"#)
        .await
        .unwrap();

    let err = LocalProviderLedger::open(&path).await.unwrap_err();

    assert!(err.to_string().contains("missing trailing newline"));
}
```

- [ ] **Step 5: Run tests and confirm fail**

Run:

```bash
cargo test -p roci-core session::ledger::tests::
```

Expected: compile failures for missing ledger types.

- [ ] **Step 6: Implement ledger record types**

In `ledger.rs`, implement:

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::agent::runtime::chat::ThreadId;
use crate::types::ModelMessage;

use super::{SessionError, SessionResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderLedgerRecord {
    Message {
        schema_version: u16,
        seq: u64,
        thread_id: ThreadId,
        message: ModelMessage,
    },
    Compacted {
        schema_version: u16,
        seq: u64,
        thread_id: ThreadId,
        replacement_history: Vec<ModelMessage>,
        replaces_through_seq: u64,
    },
    LedgerInvalidated {
        schema_version: u16,
        seq: u64,
        thread_id: ThreadId,
        latest_seq: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderLedgerState {
    pub thread_id: ThreadId,
    pub latest_seq: u64,
    pub messages: Vec<ModelMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderLedgerSnapshot {
    pub schema_version: u16,
    pub generated_at: DateTime<Utc>,
    pub threads: Vec<ProviderLedgerState>,
}

#[derive(Debug)]
pub struct LocalProviderLedger {
    path: PathBuf,
    inner: tokio::sync::Mutex<ProviderLedgerInner>,
}

#[derive(Debug)]
struct ProviderLedgerInner {
    states: HashMap<ThreadId, ProviderLedgerState>,
    next_seq: u64,
}
```

- [ ] **Step 7: Implement open/replay**

Implement:

```rust
impl LocalProviderLedger {
    pub async fn open(path: impl Into<PathBuf>) -> SessionResult<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| SessionError::io(parent, source))?;
        }
        let (states, latest_seq) = replay_provider_ledger(&path).await?;
        Ok(Self {
            path,
            inner: tokio::sync::Mutex::new(ProviderLedgerInner {
                states,
                next_seq: latest_seq + 1,
            }),
        })
    }

    pub async fn state_for_thread(&self, thread_id: ThreadId) -> SessionResult<ProviderLedgerState> {
        let inner = self.inner.lock().await;
        Ok(inner.states.get(&thread_id).cloned().unwrap_or(ProviderLedgerState {
            thread_id,
            latest_seq: 0,
            messages: Vec::new(),
        }))
    }
}
```

`replay_provider_ledger` must:

- return empty state if file missing
- ignore blank lines
- error on malformed nonblank lines with `line N`
- error on final nonblank line without trailing newline
- apply newest `Compacted` as effective base by replacing messages for that thread
- apply `Message` by pushing onto the thread messages
- apply `LedgerInvalidated` by clearing messages and setting latest seq

- [ ] **Step 8: Implement append methods**

Implement:

```rust
impl LocalProviderLedger {
    pub async fn append_message(
        &self,
        thread_id: ThreadId,
        message: ModelMessage,
    ) -> SessionResult<u64> {
        let mut inner = self.inner.lock().await;
        let seq = inner.next_seq;
        let record = ProviderLedgerRecord::Message {
            schema_version: 1,
            seq,
            thread_id,
            message: message.clone(),
        };
        self.append_record_locked(&record).await?;
        inner.next_seq += 1;
        let state = inner.states.entry(thread_id).or_insert_with(|| ProviderLedgerState {
            thread_id,
            latest_seq: 0,
            messages: Vec::new(),
        });
        state.latest_seq = seq;
        state.messages.push(message);
        Ok(seq)
    }

    pub async fn append_compacted(
        &self,
        thread_id: ThreadId,
        replaces_through_seq: u64,
        replacement_history: Vec<ModelMessage>,
    ) -> SessionResult<u64> {
        let mut inner = self.inner.lock().await;
        let seq = inner.next_seq;
        let record = ProviderLedgerRecord::Compacted {
            schema_version: 1,
            seq,
            thread_id,
            replacement_history: replacement_history.clone(),
            replaces_through_seq,
        };
        self.append_record_locked(&record).await?;
        inner.next_seq += 1;
        inner.states.insert(thread_id, ProviderLedgerState {
            thread_id,
            latest_seq: seq,
            messages: replacement_history,
        });
        Ok(seq)
    }

    pub async fn append_ledger_invalidated(
        &self,
        thread_id: ThreadId,
        latest_seq: u64,
    ) -> SessionResult<u64> {
        let mut inner = self.inner.lock().await;
        let seq = inner.next_seq;
        let record = ProviderLedgerRecord::LedgerInvalidated {
            schema_version: 1,
            seq,
            thread_id,
            latest_seq,
        };
        self.append_record_locked(&record).await?;
        inner.next_seq += 1;
        inner.states.insert(thread_id, ProviderLedgerState {
            thread_id,
            latest_seq: seq,
            messages: Vec::new(),
        });
        Ok(seq)
    }
}
```

`append_record_locked` must be called while holding `inner`. It serializes one
JSON object, appends `\n`, flushes, and `sync_data()`s, following
`JsonlAgentRuntimeEventStore`. Holding one mutex across seq allocation, file
append, and in-memory state update preserves monotonic file order.

- [ ] **Step 9: Verify ledger**

Run:

```bash
cargo test -p roci-core session::ledger::tests::
cargo fmt --all -- --check
```

Expected: all ledger tests pass.

---

## Task 3: Snapshot DTOs And Resource Manifest

**Files:**
- Create: `crates/roci-core/src/session/snapshot.rs`
- Modify: `crates/roci-core/src/session/mod.rs`
- Modify: `crates/roci-core/src/prelude.rs`
- Test: `crates/roci-core/src/session/snapshot.rs`

- [ ] **Step 1: Add snapshot module exports**

In `session/mod.rs`, add:

```rust
mod snapshot;

pub use snapshot::{
    CreateSessionOptions, ImportPolicy, ProviderLedgerSummary, RuntimeSnapshotCache,
    SessionResourceManifest, SessionResourceRef, SessionResumeState, SessionSnapshot,
};
```

In `prelude.rs`, re-export:

```rust
pub use crate::session::{
    CreateSessionOptions, ImportPolicy, SessionResumeState, SessionSnapshot,
};
```

- [ ] **Step 2: Write failing serialization test**

Create `snapshot.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::runtime::chat::AgentRuntimeEvent;
    use crate::agent::runtime::chat::{RuntimeSnapshot, ThreadId};
    use crate::session::{SessionConfig, SessionId, SessionMetadata, SessionResourceNamespace};
    use tempfile::tempdir;

    #[test]
    fn session_snapshot_serializes_manifest_without_resource_bytes() {
        let id = SessionId::parse("session-snapshot").unwrap();
        let thread_id = ThreadId::new();
        let metadata = SessionMetadata::new(id.clone(), None, None);
        let snapshot = SessionSnapshot {
            schema_version: 1,
            metadata,
            default_thread_id: thread_id,
            runtime: RuntimeSnapshot {
                schema_version: 1,
                threads: Vec::new(),
            },
            events: Vec::<AgentRuntimeEvent>::new(),
            provider_ledger: ProviderLedgerSummary {
                thread_id,
                latest_seq: 0,
                effective_history: Vec::new(),
            },
            resources: SessionResourceManifest {
                artifacts: vec![SessionResourceRef {
                    namespace: SessionResourceNamespace::Artifacts,
                    logical_path: Some("out.txt".parse().unwrap()),
                    storage_path: "artifacts/out.txt".into(),
                    len: 7,
                    updated_at: None,
                    available: false,
                }],
                ..SessionResourceManifest::default()
            },
            exported_at: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&snapshot).unwrap();

        assert!(json.contains("artifacts/out.txt"));
        assert!(!json.contains("payload bytes"));
    }
}
```

- [ ] **Step 3: Run test and confirm fail**

Run:

```bash
cargo test -p roci-core session::snapshot::tests::session_snapshot_serializes_manifest_without_resource_bytes
```

Expected: compile failures for missing DTOs.

- [ ] **Step 4: Implement DTOs**

In `snapshot.rs`, implement:

```rust
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::runtime::chat::{
    AgentRuntimeEvent, RuntimeCursor, RuntimeSnapshot, ThreadId,
};
use crate::types::ModelMessage;

use super::{
    LogicalPath, SessionConfig, SessionMetadata, SessionResourceNamespace,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CreateSessionOptions {
    pub id: Option<super::SessionId>,
    pub title: Option<String>,
    pub host_cwd: Option<PathBuf>,
    pub import_source: Option<PathBuf>,
    pub default_thread_id: Option<ThreadId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportPolicy {
    FailIfExists,
    NewId(Option<super::SessionId>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub schema_version: u16,
    pub metadata: SessionMetadata,
    pub default_thread_id: ThreadId,
    pub runtime: RuntimeSnapshot,
    pub events: Vec<AgentRuntimeEvent>,
    pub provider_ledger: ProviderLedgerSummary,
    pub resources: SessionResourceManifest,
    pub exported_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderLedgerSummary {
    pub thread_id: ThreadId,
    pub latest_seq: u64,
    pub effective_history: Vec<ModelMessage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SessionResourceManifest {
    pub plan: Option<SessionResourceRef>,
    pub workspace: Option<SessionResourceRef>,
    pub artifacts: Vec<SessionResourceRef>,
    pub temp_files: Vec<SessionResourceRef>,
    pub checkpoints: Vec<SessionResourceRef>,
    pub files: Vec<SessionResourceRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionResourceRef {
    pub namespace: SessionResourceNamespace,
    pub logical_path: Option<LogicalPath>,
    pub storage_path: PathBuf,
    pub len: u64,
    pub updated_at: Option<DateTime<Utc>>,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSnapshotCache {
    pub schema_version: u16,
    pub snapshot: RuntimeSnapshot,
    pub cursors: Vec<RuntimeCursor>,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct SessionResumeState {
    pub session_config: SessionConfig,
    pub metadata: SessionMetadata,
    pub default_thread_id: ThreadId,
    pub runtime: RuntimeSnapshot,
    pub model_messages: Vec<ModelMessage>,
    pub resources: SessionResourceManifest,
    pub events: Vec<AgentRuntimeEvent>,
    pub event_cursors: Vec<RuntimeCursor>,
    pub provider_ledger_seq: u64,
    pub(crate) lease: Arc<super::SessionLease>,
}
```

- [ ] **Step 5: Verify snapshot DTOs**

Run:

```bash
cargo test -p roci-core session::snapshot::tests::session_snapshot_serializes_manifest_without_resource_bytes
cargo fmt --all -- --check
```

Expected: pass.

---

## Task 4: Replay Runtime Events Into Projector And Normalize Stale Work

**Files:**
- Modify: `crates/roci-core/src/agent/runtime/chat/projector.rs`
- Modify: `crates/roci-core/src/agent/runtime/chat/jsonl_store.rs`
- Modify: `crates/roci-core/src/agent/runtime/chat/mod.rs`
- Test: `crates/roci-core/src/agent/runtime_tests/chat_projection.rs`

- [ ] **Step 1: Add failing projector replay test**

In `runtime_tests/chat_projection.rs`, add:

```rust
#[test]
fn projector_replays_runtime_events_into_snapshot() {
    let mut projector = ChatProjector::new(ChatRuntimeConfig::default());
    let thread_id = projector.default_thread_id();
    let queued = projector.queue_turn(vec![ModelMessage::user("hello replay")]);
    let started = projector.start_turn(queued.turn_id).unwrap();
    let completed = projector.complete_turn(queued.turn_id).unwrap();
    let mut events = queued.events;
    events.push(started);
    events.push(completed);

    let replayed = ChatProjector::from_events(
        ChatRuntimeConfig {
            default_thread_id: Some(thread_id),
            ..ChatRuntimeConfig::default()
        },
        events,
    )
    .unwrap();
    let thread = replayed.read_thread(thread_id).unwrap();

    assert_eq!(thread.turns[0].status, TurnStatus::Completed);
    assert_eq!(thread.messages[0].payload.text(), "hello replay");
}
```

- [ ] **Step 2: Run test and confirm fail**

Run:

```bash
cargo test -p roci-core --features agent projector_replays_runtime_events_into_snapshot
```

Expected: compile failure for missing `ChatProjector::from_events`.

- [ ] **Step 3: Add projector replay API**

In `projector.rs`, add:

```rust
impl ChatProjector {
    pub fn from_events(
        config: ChatRuntimeConfig,
        events: impl IntoIterator<Item = AgentRuntimeEvent>,
    ) -> Result<Self, AgentRuntimeError> {
        let mut projector = Self::new(config);
        for event in events {
            projector.apply_replayed_event(event)?;
        }
        Ok(projector)
    }

    pub fn apply_replayed_event(
        &mut self,
        event: AgentRuntimeEvent,
    ) -> Result<(), AgentRuntimeError> {
        if !self.threads.contains_key(&event.thread_id) {
            self.threads.insert(
                event.thread_id,
                ThreadState::new(event.thread_id, ChatRuntimeConfig::default().replay_capacity),
            );
        }
        self.thread_mut(event.thread_id)?.apply_event(event)
    }
}
```

Add `ThreadState::apply_replayed_event` that applies `AgentRuntimeEventPayload`
to snapshot without generating a new event and updates `last_seq`. Reuse the
same snapshot mutation helpers used by live projection.

Replay must cover these payload families explicitly:

- Turn lifecycle: queue/start/complete/fail/cancel by id, preserving `last_seq`.
- Message lifecycle: created/delta/completed/canceled by message id.
- Tool lifecycle: started/progress/completed/canceled by tool id.
- Approval lifecycle: requested/resolved/canceled by approval id.
- Human interaction lifecycle: requested/resolved/canceled by interaction id.
- Resource lifecycle: file/artifact/temp/checkpoint/plan/workspace events using the existing resource projection paths.

Add one regression test per family when local helper constructors exist. If a
payload variant has no public constructor, build the minimal event struct
directly in `chat_projection.rs` and assert the projected snapshot field, not
serde shape.

Add a non-default thread replay regression:

```rust
#[test]
fn projector_replays_non_default_thread_events() {
    let non_default = ThreadId::new();
    let mut source = ChatProjector::new(ChatRuntimeConfig {
        default_thread_id: Some(non_default),
        ..ChatRuntimeConfig::default()
    });
    let queued = source.queue_turn(vec![ModelMessage::user("other thread")]);

    let replayed = ChatProjector::from_events(ChatRuntimeConfig::default(), queued.events)
        .unwrap();

    assert_eq!(
        replayed.read_thread(non_default).unwrap().messages[0].payload.text(),
        "other thread"
    );
}
```

- [ ] **Step 4: Add normalization helper**

In `jsonl_store.rs`, add all-events replay for store consumers:

```rust
impl JsonlAgentRuntimeEventStore {
    pub async fn all_events(&self) -> Vec<AgentRuntimeEvent> {
        let inner = self.inner.lock().await;
        let mut events = inner
            .threads
            .values()
            .flat_map(|thread| thread.events.iter().cloned())
            .collect::<Vec<_>>();
        events.sort_by_key(|event| (event.thread_id.to_string(), event.seq));
        events
    }
}
```

Do not sort by timestamp. Canonical replay order is per-thread `seq`; imported
or old events can have timestamps that do not match append order.

Add:

```rust
#[tokio::test]
async fn all_events_orders_by_thread_seq_not_timestamp() {
    let dir = tempdir().unwrap();
    let store = JsonlAgentRuntimeEventStore::open(dir.path().join("events.jsonl")).unwrap();
    let mut projector = ChatProjector::new(ChatRuntimeConfig::default());
    let queued = projector.queue_turn(vec![ModelMessage::user("timestamp order")]);
    let mut first = queued.events[0].clone();
    let mut second = queued.events[1].clone();
    first.timestamp = chrono::Utc::now();
    second.timestamp = first.timestamp - chrono::Duration::seconds(60);
    store.append(first.clone()).await.unwrap();
    store.append(second.clone()).await.unwrap();

    let events = store.all_events().await;

    assert_eq!(events.iter().map(|event| event.seq).collect::<Vec<_>>(), vec![1, 2]);
}
```

- [ ] **Step 5: Add normalization helper**

Add:

```rust
impl ChatProjector {
    pub fn thread_ids(&self) -> Vec<ThreadId> {
        let mut ids = self.threads.keys().copied().collect::<Vec<_>>();
        ids.sort_by_key(|id| id.to_string());
        ids
    }

    pub fn normalize_for_resume(&mut self) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
        let mut events = Vec::new();
        for thread_id in self.thread_ids() {
            events.extend(self.normalize_thread_for_resume(thread_id)?);
        }
        Ok(events)
    }
}
```

Normalization must emit the same payload types live cancellation uses:

- `ApprovalCanceled`
- `HumanInteractionCanceled`
- `TurnCanceled`

For active tools/streaming messages, emit existing payloads only:

- `ToolCompleted` with `AgentToolResult { is_error: true, result: {"error":"session resumed before tool completed"} }`
- `MessageCompleted` with the last projected message payload

Do not add a new payload type in `.3`.

Add normalization regression tests named:

- `normalize_for_resume_cancels_queued_and_running_turns`: queue two turns, start one, normalize, assert returned events contain two `TurnCanceled` payloads and snapshot turns are canceled.
- `normalize_for_resume_cancels_pending_approval_and_human_interaction`: project one pending approval and one pending human interaction, normalize, assert returned events contain `ApprovalCanceled` and `HumanInteractionCanceled`.
- `normalize_for_resume_finishes_active_tool_with_error_result`: project an active tool, normalize, assert returned event contains `ToolCompleted` with `is_error=true`.
- `normalize_for_resume_completes_streaming_message_with_current_payload`: project a message delta without completion, normalize, assert returned event contains `MessageCompleted` with the current projected payload.

- [ ] **Step 6: Verify projector slice**

Run:

```bash
cargo test -p roci-core --features agent "agent::runtime::tests::chat_projection"
cargo test -p roci-core --features agent projector_replays_runtime_events_into_snapshot
cargo test -p roci-core --features agent projector_replays_non_default_thread_events
cargo test -p roci-core --features agent all_events_orders_by_thread_seq_not_timestamp
```

Expected: pass.

---

## Task 5: LocalSessionStore Create/Open And Caches

**Files:**
- Create: `crates/roci-core/src/session/store.rs`
- Modify: `crates/roci-core/src/session/mod.rs`
- Modify: `crates/roci-core/src/session/resources.rs`
- Test: `crates/roci-core/src/agent/runtime_tests/session.rs`

- [ ] **Step 1: Export store**

In `session/mod.rs`, add:

```rust
mod store;

pub use store::{LocalSessionStore, SessionLease};
```

- [ ] **Step 2: Add failing create/open metadata tests**

In `runtime_tests/session.rs`, add:

```rust
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
    assert_eq!(reopened.metadata.host_cwd, Some(PathBuf::from("/tmp/project")));
    assert!(SessionConfig::new(id, sessions.path())
        .conventions()
        .provider_ledger_file()
        .is_file());
}
```

- [ ] **Step 3: Add failing cache-corruption tests**

Add:

```rust
#[tokio::test]
async fn local_session_store_ignores_corrupt_snapshot_caches_when_logs_replay() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-cache");
    let created = store
        .create(CreateSessionOptions {
            id: Some(id.clone()),
            ..CreateSessionOptions::default()
        })
        .await
        .unwrap();
    let conventions = created.session_config.conventions();
    tokio::fs::write(conventions.runtime_snapshot_file(), b"not-json")
        .await
        .unwrap();
    tokio::fs::write(conventions.provider_ledger_snapshot_file(), b"not-json")
        .await
        .unwrap();
    drop(created);

    let reopened = store.open(id).await.expect("canonical logs should replay");

    assert!(reopened.runtime.threads.len() <= 1);
    assert!(conventions.runtime_snapshot_file().is_file());
}
```

- [ ] **Step 4: Add failing event replay open test**

Add:

```rust
#[tokio::test]
async fn session_open_replays_events_into_runtime_snapshot() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-replay");
    let state = store
        .create(CreateSessionOptions {
            id: Some(id.clone()),
            ..CreateSessionOptions::default()
        })
        .await
        .unwrap();
    let registry = registry_with_streaming_provider("stub", 1, 1);
    let mut config = test_agent_config();
    config.model = "stub:session".parse().unwrap();
    let agent = AgentRuntime::resume_session(registry, test_config(), config, state)
        .await
        .expect("session resumes");
    let turn_id = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("hello replay")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: None,
        })
        .await
        .expect("turn queues");
    assert_turn_status(&agent, turn_id, TurnStatus::Completed).await;
    drop(agent);

    let reopened = store.open(id).await.expect("session opens");

    assert_eq!(reopened.runtime.threads.len(), 1);
    assert_eq!(reopened.runtime.threads[0].turns[0].status, TurnStatus::Completed);
    assert!(reopened.runtime.threads[0]
        .messages
        .iter()
        .any(|message| message.payload.text() == "hello replay"));
}
```

- [ ] **Step 5: Add failing lease test**

Add:

```rust
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
```

- [ ] **Step 6: Add failing corrupt event mapping test**

Add:

```rust
#[tokio::test]
async fn local_session_store_open_wraps_corrupt_events_with_path_and_line() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-corrupt-events");
    let state = store
        .create(CreateSessionOptions {
            id: Some(id.clone()),
            ..CreateSessionOptions::default()
        })
        .await
        .unwrap();
    let events_file = state.session_config.conventions().events_file();
    drop(state);
    tokio::fs::write(&events_file, b"not-json\n").await.unwrap();

    let err = store.open(id).await.unwrap_err();
    let message = err.to_string();

    assert!(message.contains("events.jsonl"));
    assert!(message.contains("line 1"));
}
```

- [ ] **Step 7: Add failing corrupt provider ledger mapping test**

Add:

```rust
#[tokio::test]
async fn local_session_store_open_wraps_corrupt_provider_ledger_with_path_and_line() {
    let sessions = tempdir().unwrap();
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-corrupt-ledger");
    let state = store
        .create(CreateSessionOptions { id: Some(id.clone()), ..Default::default() })
        .await
        .unwrap();
    let ledger_path = state.session_config.conventions().provider_ledger_file();
    drop(state);
    tokio::fs::write(&ledger_path, b"not-json\n").await.unwrap();

    let err = store.open(id).await.unwrap_err();
    let message = err.to_string();

    assert!(message.contains("model_messages.jsonl"));
    assert!(message.contains("line 1"));
}
```

- [ ] **Step 8: Run tests and confirm fail**

Run:

```bash
cargo test -p roci-core --features agent local_session_store_create_writes_metadata_once_and_open_preserves_it
cargo test -p roci-core --features agent local_session_store_ignores_corrupt_snapshot_caches_when_logs_replay
cargo test -p roci-core --features agent session_open_replays_events_into_runtime_snapshot
cargo test -p roci-core --features agent local_session_store_rejects_second_open_while_resume_state_alive
cargo test -p roci-core --features agent local_session_store_open_wraps_corrupt_events_with_path_and_line
cargo test -p roci-core --features agent local_session_store_open_wraps_corrupt_provider_ledger_with_path_and_line
```

Expected: compile failure for `LocalSessionStore`.

- [ ] **Step 9: Implement `LocalSessionStore` skeleton**

In `store.rs`, implement:

```rust
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::Utc;

use crate::agent::runtime::chat::{ChatProjector, ChatRuntimeConfig};

use super::{
    CreateSessionOptions, ImportPolicy, LocalProviderLedger, LocalSessionFs,
    LocalSessionResources, PathConventions, ProviderLedgerSummary, RuntimeSnapshotCache,
    SessionConfig, SessionError, SessionMetadata, SessionResourceManifest, SessionResult,
    SessionResumeState, SessionSnapshot,
};

#[derive(Debug, Clone)]
pub struct LocalSessionStore {
    root: PathBuf,
}

#[derive(Debug)]
pub struct SessionLease {
    key: PathBuf,
}

static ACTIVE_SESSION_LEASES: OnceLock<Mutex<std::collections::HashSet<PathBuf>>> =
    OnceLock::new();

impl LocalSessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    async fn acquire_lease(&self, config: &SessionConfig) -> SessionResult<Arc<SessionLease>> {
        let key = config.conventions().root().to_path_buf();
        let leases = ACTIVE_SESSION_LEASES.get_or_init(Default::default);
        let mut guard = leases.lock().expect("session lease registry poisoned");
        if !guard.insert(key.clone()) {
            return Err(SessionError::AlreadyOpen { path: key });
        }
        Ok(Arc::new(SessionLease { key }))
    }
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        if let Some(leases) = ACTIVE_SESSION_LEASES.get() {
            if let Ok(mut guard) = leases.lock() {
                guard.remove(&self.key);
            }
        }
    }
}
```

- [ ] **Step 10: Implement create**

Create must:

1. Resolve id: options id or `SessionId::new_v4()`.
2. Build `SessionConfig::new(id.clone(), &self.root)`.
3. Acquire `SessionLease` for the target session root.
4. Return `AlreadyExists` if session root exists.
5. Create dirs through `LocalSessionFs::with_conventions` and `LocalSessionResources::with_conventions`.
6. Write metadata once with `metadata.title = options.title`, preserving `host_cwd` and `import_source`.
7. Create empty `events.jsonl`, `model_messages.jsonl`, and snapshot cache files with valid empty JSON where applicable.
8. Return the acquired `SessionLease` inside `SessionResumeState`.

Use:

```rust
let default_thread_id = options.default_thread_id.unwrap_or_default();
let projector = ChatProjector::new(ChatRuntimeConfig {
    default_thread_id: Some(default_thread_id),
    ..ChatRuntimeConfig::default()
});
let runtime = projector.read_snapshot();
```

- [ ] **Step 11: Implement open**

Open must:

1. Build `SessionConfig::new(id.clone(), &self.root)` and acquire `SessionLease`.
2. Read metadata.
3. Validate `metadata.id == config.id`; return `RuntimeProjection` or `InvalidMetadata` if mismatched.
4. Open strict `JsonlAgentRuntimeEventStore`.
5. Rebuild runtime snapshot from `events.jsonl` by calling `JsonlAgentRuntimeEventStore::all_events()` and `ChatProjector::from_events`; store the normalized event vector in `SessionResumeState.events`.
6. If snapshot has nonterminal work, normalize and append cancel events.
7. Open `LocalProviderLedger` and read default thread state.
8. Build resource manifest from existing files.
9. Write snapshot caches atomically.
10. Return `SessionResumeState` holding the acquired `SessionLease`.

Atomic cache write helper:

```rust
async fn write_json_atomic<T: serde::Serialize>(
    path: &Path,
    value: &T,
) -> SessionResult<()> {
    let tmp = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| {
        SessionError::InvalidMetadata {
            path: path.to_path_buf(),
            message: source.to_string(),
        }
    })?;
    tokio::fs::write(&tmp, bytes)
        .await
        .map_err(|source| SessionError::io(&tmp, source))?;
    tokio::fs::rename(&tmp, path)
        .await
        .map_err(|source| SessionError::io(path, source))
}
```

- [ ] **Step 12: Implement resource manifest builder and scrubber**

In `store.rs`, add:

```rust
async fn build_resource_manifest(
    conventions: &PathConventions,
) -> SessionResult<SessionResourceManifest> {
    Ok(SessionResourceManifest {
        plan: resource_ref_if_exists(
            SessionResourceNamespace::Plan,
            None,
            conventions.root(),
            conventions.plan_file(),
        )
        .await?,
        workspace: resource_ref_if_exists(
            SessionResourceNamespace::Workspace,
            None,
            conventions.root(),
            conventions.workspace_file(),
        )
        .await?,
        artifacts: resource_refs_in_dir(
            SessionResourceNamespace::Artifacts,
            conventions.root(),
            conventions.artifacts_dir(),
        )
        .await?,
        temp_files: resource_refs_in_dir(
            SessionResourceNamespace::Temp,
            conventions.root(),
            conventions.temp_dir(),
        )
        .await?,
        checkpoints: resource_refs_in_dir(
            SessionResourceNamespace::Checkpoints,
            conventions.root(),
            conventions.checkpoints_dir(),
        )
        .await?,
        files: resource_refs_in_dir(
            SessionResourceNamespace::Files,
            conventions.root(),
            conventions.files_dir(),
        )
        .await?,
    })
}
```

Each `SessionResourceRef.storage_path` must be relative to
`conventions.root()`, `len` comes from metadata length, `updated_at` comes from
modified time when available, and `available=true` only when the file exists.
`open` must merge two sources:

- replayed runtime resource events, preserving manifest entries even when the
  payload file is missing
- filesystem scan, setting `available=true` and real `len` for files that exist

Only projected `ThreadSnapshot.resources` should omit missing payloads. The
`SessionResumeState.resources` manifest must retain unavailable imported refs so
export-after-import does not lose them.

Add tests:

- `resource_manifest_includes_all_session_namespaces`: create one file in each namespace and assert manifest contains `plan.md`, `workspace.yaml`, `files/...`, `artifacts/...`, `tmp/...`, and `checkpoints/...`.
- `resource_manifest_scrubber_marks_imported_missing_resources_unavailable`: pass imported refs through the scrubber for an empty target session and assert every unavailable payload has `available=false`.
- `resource_manifest_preserves_unavailable_refs_from_replayed_events`: import/replay a resource event for `artifacts/missing.txt` without the file present and assert `SessionResumeState.resources.artifacts` contains that ref with `available=false`.

- [ ] **Step 13: Verify store create/open**

Run:

```bash
cargo test -p roci-core --features agent local_session_store_create_writes_metadata_once_and_open_preserves_it
cargo test -p roci-core --features agent local_session_store_ignores_corrupt_snapshot_caches_when_logs_replay
cargo test -p roci-core --features agent session_open_replays_events_into_runtime_snapshot
cargo test -p roci-core --features agent local_session_store_rejects_second_open_while_resume_state_alive
cargo test -p roci-core --features agent local_session_store_open_wraps_corrupt_events_with_path_and_line
cargo test -p roci-core --features agent local_session_store_open_wraps_corrupt_provider_ledger_with_path_and_line
cargo fmt --all -- --check
```

Expected: pass.

---

## Task 6: Runtime Constructor Boundary And Resume API

**Files:**
- Modify: `crates/roci-core/src/agent/runtime.rs`
- Modify: `crates/roci-core/src/agent/runtime/config.rs`
- Modify: `crates/roci-core/src/agent/runtime_tests/session.rs`
- Modify examples that call `AgentRuntime::new` with session config.

- [ ] **Step 1: Add failing metadata overwrite regression**

In `runtime_tests/session.rs`, add:

```rust
#[tokio::test]
async fn agent_runtime_try_new_does_not_overwrite_existing_session_metadata() {
    let sessions = tempdir().expect("session tempdir should be created");
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-no-overwrite");
    let created = store
        .create(CreateSessionOptions {
            id: Some(id.clone()),
            host_cwd: Some(PathBuf::from("/tmp/original")),
            ..CreateSessionOptions::default()
        })
        .await
        .unwrap();
    let before = created.metadata.clone();
    drop(created);

    let registry = registry_with_streaming_provider("stub", 1, 1);
    let mut config = test_agent_config();
    config.model = "stub:session".parse().unwrap();
    config.session = Some(SessionConfig::new(id.clone(), sessions.path()));
    let _agent = AgentRuntime::try_new(registry, test_config(), config).unwrap();

    let after = SessionMetadata::read_from_path(
        SessionConfig::new(id, sessions.path()).conventions().metadata_file(),
    )
    .unwrap();

    assert_eq!(after.created_at, before.created_at);
    assert_eq!(after.host_cwd, Some(PathBuf::from("/tmp/original")));
}
```

- [ ] **Step 2: Add failing resume test**

Add:

```rust
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
    let ledger = LocalProviderLedger::open(provider_ledger_file)
        .await
        .unwrap();
    let persisted = ModelMessage::user("persisted");
    ledger
        .append_message(thread_id, persisted.clone())
        .await
        .unwrap();
    drop(ledger);
    let state = store.open(id).await.unwrap();
    let registry = registry_with_streaming_provider("stub", 1, 1);
    let mut config = test_agent_config();
    config.model = "stub:session".parse().unwrap();

    let agent = AgentRuntime::resume_session(registry, test_config(), config, state)
        .await
        .expect("session resumes");

    assert_eq!(agent.messages().await, vec![persisted]);
}
```

- [ ] **Step 3: Run tests and confirm fail**

Run:

```bash
cargo test -p roci-core --features agent agent_runtime_try_new_does_not_overwrite_existing_session_metadata
cargo test -p roci-core --features agent resume_session_seeds_runtime_snapshot_and_provider_ledger
```

Expected: first fails with metadata overwrite if old behavior remains; second compile fails for missing `resume_session`.

- [ ] **Step 4: Remove constructor session writes**

In `runtime.rs`, remove `SessionMetadata::new(...).write_to_path(...)` from `new_inner`.

Add `LocalSessionFs::open_existing_with_conventions` and
`LocalSessionResources::open_existing_with_conventions`. Runtime constructors
must call only those open-existing variants for configured sessions. Creation
variants remain owned by `LocalSessionStore::create`.

- [ ] **Step 5: Add `resume_session`**

In `runtime.rs`, implement:

```rust
pub async fn resume_session(
    registry: Arc<ProviderRegistry>,
    roci_config: RociConfig,
    mut config: AgentConfig,
    state: crate::session::SessionResumeState,
) -> Result<Self, RociError> {
    if state.session_config.id != state.metadata.id {
        return Err(RociError::InvalidState(
            "resume state session id does not match metadata id".into(),
        ));
    }
    if let Some(existing) = &config.session {
        if existing != &state.session_config {
            return Err(RociError::InvalidState(
                "resume session config does not match resume state".into(),
            ));
        }
    }
    if let Some(thread_id) = config.chat.default_thread_id {
        if thread_id != state.default_thread_id {
            return Err(RociError::InvalidState(
                "resume default thread id does not match resume state".into(),
            ));
        }
    }

    config.session = Some(state.session_config.clone());
    config.chat.default_thread_id = Some(state.default_thread_id);
    let agent = Self::try_new(registry, roci_config, config)?;
    agent.import_runtime_snapshot(state.runtime, state.model_messages).await?;
    agent.hold_session_lease(state.lease);
    Ok(agent)
}
```

Add private `import_runtime_snapshot` that sets chat projector state and `messages` without invalidating event store. Prefer a projector method `import_runtime_snapshot(RuntimeSnapshot)`.
Add private `hold_session_lease(Arc<SessionLease>)` that stores the lease on
`AgentRuntime`, so a live resumed runtime blocks a second writer.

Add mismatch regression tests:

- `resume_session_rejects_state_metadata_id_mismatch`: mutate a test-only `SessionResumeState` clone so `metadata.id != session_config.id`, then assert `InvalidState`.
- `resume_session_rejects_config_session_mismatch`: pass `config.session = Some(SessionConfig::new(other_id, sessions.path()))`, then assert `InvalidState`.
- `resume_session_rejects_default_thread_mismatch`: pass `config.chat.default_thread_id = Some(ThreadId::new())`, then assert `InvalidState`.

- [ ] **Step 6: Update existing session tests/helpers for constructor boundary**

Update helpers in `runtime_tests/session.rs` that currently expect
`AgentRuntime::try_new` to create session files. Any helper named like
`runtime_with_session` should create/open through `LocalSessionStore` and call
`AgentRuntime::resume_session`.

Update these existing tests:

- `session_config_uses_jsonl_store_without_project_cwd_storage`: assert cwd stays clean after `LocalSessionStore::create` + `resume_session`, not constructor side effects.
- `plan_updates_are_mirrored_to_plan_md`: build runtime through resumed state before calling plan mutation APIs.
- `runtime_resource_methods_write_files_events_and_snapshot`: build runtime through resumed state before resource writes.
- Any corrupt-constructor test should move to `LocalSessionStore::open` or to `try_new` with precreated corrupt session files, depending on whether the assertion is about store replay or runtime open-existing behavior.

- [ ] **Step 7: Verify runtime boundary**

Run:

```bash
cargo test -p roci-core --features agent agent_runtime_try_new_does_not_overwrite_existing_session_metadata
cargo test -p roci-core --features agent resume_session_seeds_runtime_snapshot_and_provider_ledger
cargo test -p roci-core --features agent "agent::runtime::tests::session"
```

Expected: pass.

---

## Task 7: Runtime Provider Ledger Write Hooks

**Files:**
- Modify: `crates/roci-core/src/agent/runtime.rs`
- Modify: `crates/roci-core/src/agent/runtime/run_loop.rs`
- Modify: `crates/roci-core/src/agent/runtime/lifecycle.rs`
- Modify: `crates/roci-core/src/agent/runtime/mutations.rs`
- Test: `crates/roci-core/src/agent/runtime_tests/session.rs`

- [ ] **Step 1: Add failing committed-turn ledger test**

In `runtime_tests/session.rs`, add:

```rust
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
    config.model = "stub:session".parse().unwrap();
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
```

- [ ] **Step 2: Add failing replace/import/reset tests**

Add:

```rust
#[tokio::test]
async fn replace_messages_writes_compacted_provider_ledger() {
    let sessions = tempdir().unwrap();
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-ledger-replace");
    let state = store
        .create(CreateSessionOptions { id: Some(id.clone()), ..Default::default() })
        .await
        .unwrap();
    let agent = AgentRuntime::resume_session(test_registry(), test_config(), test_agent_config(), state)
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
```

Add no-duplicate resume regression:

```rust
#[tokio::test]
async fn resumed_history_is_not_appended_again_on_next_turn() {
    let sessions = tempdir().unwrap();
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-ledger-no-duplicates");
    let state = store
        .create(CreateSessionOptions { id: Some(id.clone()), ..Default::default() })
        .await
        .unwrap();
    let thread_id = state.default_thread_id;
    let ledger_path = state.session_config.conventions().provider_ledger_file();
    drop(state);
    let persisted = ModelMessage::user("persisted before resume");
    let ledger = LocalProviderLedger::open(ledger_path).await.unwrap();
    ledger.append_message(thread_id, persisted.clone()).await.unwrap();
    drop(ledger);

    let state = store.open(id.clone()).await.unwrap();
    let registry = registry_with_streaming_provider("stub", 1, 1);
    let mut config = test_agent_config();
    config.model = "stub:session".parse().unwrap();
    let agent = AgentRuntime::resume_session(registry, test_config(), config, state)
        .await
        .unwrap();
    let turn_id = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("new prompt")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: None,
        })
        .await
        .unwrap();
    assert_turn_status(&agent, turn_id, TurnStatus::Completed).await;
    drop(agent);

    let reopened = store.open(id).await.unwrap();
    let user_texts = reopened
        .model_messages
        .iter()
        .filter(|message| message.role == Role::User)
        .map(ModelMessage::text)
        .collect::<Vec<_>>();

    assert_eq!(
        user_texts,
        vec![
            "persisted before resume".to_string(),
            "new prompt".to_string(),
        ]
    );
}
```

Add failed-provider regression:

- `failed_provider_turn_does_not_append_unaccepted_provider_messages`: register a
  test provider whose `stream_text` returns `Err(RociError::Provider(...))`
  before emitting any text, enqueue one user message, wait for `TurnStatus::Failed`,
  drop runtime, reopen session, and assert `model_messages.is_empty()`.

- [ ] **Step 3: Run tests and confirm fail**

Run:

```bash
cargo test -p roci-core --features agent completed_turn_appends_provider_ledger_messages
cargo test -p roci-core --features agent replace_messages_writes_compacted_provider_ledger
cargo test -p roci-core --features agent resumed_history_is_not_appended_again_on_next_turn
cargo test -p roci-core --features agent failed_provider_turn_does_not_append_unaccepted_provider_messages
```

Expected: fail because runtime does not append provider ledger.

- [ ] **Step 4: Add runtime ledger handle**

In `AgentRuntime`, add:

```rust
provider_ledger: Option<Arc<crate::session::LocalProviderLedger>>,
persisted_provider_message_count: Arc<tokio::sync::Mutex<usize>>,
session_lease: Option<Arc<crate::session::SessionLease>>,
```

Populate `provider_ledger` and `session_lease` from `SessionResumeState` in
`resume_session`; leave them `None` for non-session runtime. Seed
`persisted_provider_message_count` from `state.model_messages.len()` before
moving messages into the runtime so resumed messages are not appended again.

- [ ] **Step 5: Append ledger after successful run**

In `run_loop.rs`, after provider run result has committed `self.messages` and
before `TurnCompleted`, append any new provider ledger messages for default
thread. Use `persisted_provider_message_count`; never append the entire resumed
history again.

```rust
async fn persist_provider_ledger_messages(
    &self,
    thread_id: ThreadId,
    messages: &[ModelMessage],
) -> Result<(), RociError> {
    let Some(ledger) = &self.provider_ledger else {
        return Ok(());
    };
    let mut persisted = self.persisted_provider_message_count.lock().await;
    for message in &messages[*persisted..] {
        ledger
            .append_message(thread_id, message.clone())
            .await
            .map_err(|err| RociError::InvalidState(err.to_string()))?;
    }
    *persisted = messages.len();
    Ok(())
}
```

- [ ] **Step 6: Append compacted records for mutations**

In `replace_messages` and `import_thread`, after in-memory message replace succeeds:

```rust
if let Some(ledger) = &self.provider_ledger {
    let thread_id = self.default_thread_id();
    let latest_seq = ledger
        .state_for_thread(thread_id)
        .await
        .map_err(|err| RociError::InvalidState(err.to_string()))?
        .latest_seq;
    ledger
        .append_compacted(thread_id, latest_seq, messages.clone())
        .await
        .map_err(|err| RociError::InvalidState(err.to_string()))?;
    *self.persisted_provider_message_count.lock().await = messages.len();
}
```

In `reset`, append `ledger_invalidated` and reset persisted message count to `0`.

- [ ] **Step 7: Verify ledger hooks**

Run:

```bash
cargo test -p roci-core --features agent completed_turn_appends_provider_ledger_messages
cargo test -p roci-core --features agent replace_messages_writes_compacted_provider_ledger
cargo test -p roci-core --features agent resumed_history_is_not_appended_again_on_next_turn
cargo test -p roci-core --features agent failed_provider_turn_does_not_append_unaccepted_provider_messages
cargo test -p roci-core --features agent "agent::runtime::tests::session"
```

Expected: pass.

---

## Task 8: Import/Export Snapshot APIs

**Files:**
- Modify: `crates/roci-core/src/session/store.rs`
- Test: `crates/roci-core/src/agent/runtime_tests/session.rs`

- [ ] **Step 1: Add failing export/import tests**

In `runtime_tests/session.rs`, add:

```rust
#[tokio::test]
async fn export_snapshot_contains_manifest_and_no_resource_bytes() {
    let sessions = tempdir().unwrap();
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-export");
    let state = store
        .create(CreateSessionOptions { id: Some(id.clone()), ..Default::default() })
        .await
        .unwrap();
    let resources = LocalSessionResources::with_conventions(state.session_config.conventions())
        .unwrap();
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
async fn import_snapshot_new_id_omits_unavailable_resources_from_runtime_manifest() {
    let sessions = tempdir().unwrap();
    let source_store = LocalSessionStore::new(sessions.path().join("source"));
    let target_store = LocalSessionStore::new(sessions.path().join("target"));
    let source_id = session_id("source-session");
    let source = source_store
        .create(CreateSessionOptions { id: Some(source_id.clone()), ..Default::default() })
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
    assert_eq!(
        imported.resources.artifacts[0].storage_path,
        PathBuf::from("artifacts/artifact.txt")
    );
    assert!(!imported.resources.artifacts[0].available);
    assert!(imported.runtime.threads.iter().all(|thread| {
        thread.resources.artifacts.is_empty()
    }));
}
```

Add export-after-import preservation test:

```rust
#[tokio::test]
async fn export_after_manifest_import_preserves_unavailable_resource_refs() {
    let sessions = tempdir().unwrap();
    let source_store = LocalSessionStore::new(sessions.path().join("source"));
    let target_store = LocalSessionStore::new(sessions.path().join("target"));
    let source_id = session_id("source-export-import");
    let source = source_store
        .create(CreateSessionOptions { id: Some(source_id.clone()), ..Default::default() })
        .await
        .unwrap();
    LocalSessionResources::with_conventions(source.session_config.conventions())
        .unwrap()
        .write_artifact(logical_path("artifact.txt"), b"payload")
        .unwrap();
    drop(source);
    let snapshot = source_store.export_snapshot(source_id).await.unwrap();
    let target_id = session_id("target-export-import");
    let imported = target_store
        .import_snapshot(snapshot, ImportPolicy::NewId(Some(target_id.clone())))
        .await
        .unwrap();
    drop(imported);

    let exported = target_store.export_snapshot(target_id).await.unwrap();

    assert_eq!(exported.resources.artifacts.len(), 1);
    assert_eq!(
        exported.resources.artifacts[0].storage_path,
        PathBuf::from("artifacts/artifact.txt")
    );
    assert!(!exported.resources.artifacts[0].available);
}
```

- [ ] **Step 2: Add failing conflict test**

Add:

```rust
#[tokio::test]
async fn import_snapshot_fail_if_exists_rejects_existing_target() {
    let sessions = tempdir().unwrap();
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-import-conflict");
    let created = store
        .create(CreateSessionOptions { id: Some(id.clone()), ..Default::default() })
        .await
        .unwrap();
    drop(created);
    let snapshot = store.export_snapshot(id).await.unwrap();

    let err = store
        .import_snapshot(snapshot, ImportPolicy::FailIfExists)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("already exists"));
}
```

- [ ] **Step 3: Run tests and confirm fail**

Run:

```bash
cargo test -p roci-core --features agent export_snapshot_contains_manifest_and_no_resource_bytes
cargo test -p roci-core --features agent import_snapshot_new_id_omits_unavailable_resources_from_runtime_manifest
cargo test -p roci-core --features agent export_after_manifest_import_preserves_unavailable_resource_refs
cargo test -p roci-core --features agent import_snapshot_fail_if_exists_rejects_existing_target
```

Expected: fail until import/export implemented.

- [ ] **Step 4: Implement `export_snapshot`**

In `store.rs`, implement:

```rust
pub async fn export_snapshot(&self, id: SessionId) -> SessionResult<SessionSnapshot> {
    let state = self.open(id).await?;
    Ok(SessionSnapshot {
        schema_version: 1,
        metadata: state.metadata,
        default_thread_id: state.default_thread_id,
        runtime: state.runtime,
        events: state.events,
        provider_ledger: ProviderLedgerSummary {
            thread_id: state.default_thread_id,
            latest_seq: state.provider_ledger_seq,
            effective_history: state.model_messages,
        },
        resources: state.resources,
        exported_at: Utc::now(),
    })
}
```

- [ ] **Step 5: Implement `import_snapshot`**

Implement:

1. Resolve target id from `ImportPolicy`.
2. Reject existing target for both policies.
3. Create new session with `CreateSessionOptions` and hold the returned state/lease while writing import files.
4. Write provider ledger compacted record with `snapshot.provider_ledger.effective_history`.
5. Write `events.jsonl` by serializing `snapshot.events` in seq order, with a trailing newline for each record.
6. Write imported resource refs into canonical semantic resource events when missing from `snapshot.events`, preserving refs with `available=false` and no payload bytes.
7. Mark all imported resource refs `available=false` unless corresponding local payload exists.
8. Build and return `SessionResumeState` from canonical replay while reusing the existing create lease; do not call `open(target_id)` while the created state is alive.

- [ ] **Step 6: Verify import/export**

Run:

```bash
cargo test -p roci-core --features agent export_snapshot_contains_manifest_and_no_resource_bytes
cargo test -p roci-core --features agent import_snapshot_new_id_omits_unavailable_resources_from_runtime_manifest
cargo test -p roci-core --features agent export_after_manifest_import_preserves_unavailable_resource_refs
cargo test -p roci-core --features agent import_snapshot_fail_if_exists_rejects_existing_target
```

Expected: pass.

---

## Task 9: Resume Provider Payload Proof

**Files:**
- Modify: `crates/roci-core/src/agent/runtime_tests/session.rs`

- [ ] **Step 1: Add failing payload proof test**

In `runtime_tests/session.rs`, add:

```rust
#[tokio::test]
async fn resumed_provider_request_uses_replayed_provider_ledger() {
    let sessions = tempdir().unwrap();
    let store = LocalSessionStore::new(sessions.path());
    let id = session_id("session-provider-ledger");
    let created = store
        .create(CreateSessionOptions { id: Some(id.clone()), ..Default::default() })
        .await
        .unwrap();
    let thread_id = created.default_thread_id;
    let ledger = LocalProviderLedger::open(created.session_config.conventions().provider_ledger_file())
        .await
        .unwrap();
    ledger
        .append_message(thread_id, ModelMessage::user("persisted context"))
        .await
        .unwrap();
    drop(ledger);
    drop(created);
    let state = store.open(id).await.unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let seen_callback = seen.clone();
    let mut config = test_agent_config();
    config.model = "stub:session".parse().unwrap();
    config.provider_payload_callback = Some(std::sync::Arc::new(move |payload| {
        seen_callback.lock().unwrap().push(payload.clone());
    }));
    let agent = AgentRuntime::resume_session(
        registry_with_streaming_provider("stub", 1, 1),
        test_config(),
        config,
        state,
    )
    .await
    .unwrap();

    let turn_id = agent
        .enqueue_turn(EnqueueTurnRequest {
            messages: vec![ModelMessage::user("new prompt")],
            generation_settings: None,
            approval_policy: None,
            collaboration_mode: None,
        })
        .await
        .unwrap();
    assert_turn_status(&agent, turn_id, TurnStatus::Completed).await;

    let payloads = seen.lock().unwrap();
    let payload = serde_json::to_string(&payloads[0]).unwrap();
    assert!(payload.contains("persisted context"));
    assert!(payload.contains("new prompt"));
}
```

- [ ] **Step 2: Run test and confirm pass**

Run:

```bash
cargo test -p roci-core --features agent resumed_provider_request_uses_replayed_provider_ledger
```

Expected: pass after Task 6/7.

---

## Task 10: CLI Compatibility And Live Smoke

**Files:**
- Modify: `crates/roci-cli/src/chat.rs`
- Test: command gates below.

- [ ] **Step 1: Update CLI chat session construction**

In `crates/roci-cli/src/chat.rs`, find the path that builds `AgentConfig.session`
from `--session-root` / `--session-id`. Replace direct runtime construction for
session-backed chat with:

```rust
let runtime = if let Some(session_config) = agent_config.session.clone() {
    let store = LocalSessionStore::new(session_config.root.clone());
    let state = if session_config.conventions().metadata_file().exists() {
        store
            .open(session_config.id.clone())
            .await
            .map_err(|err| anyhow::anyhow!(err))?
    } else {
        store
            .create(CreateSessionOptions {
                id: Some(session_config.id.clone()),
                title: None,
                host_cwd: std::env::current_dir().ok(),
                import_source: None,
                default_thread_id: agent_config.chat.default_thread_id,
            })
            .await
            .map_err(|err| anyhow::anyhow!(err))?
    };
    AgentRuntime::resume_session(registry, roci_config, agent_config, state).await?
} else {
    AgentRuntime::new(registry, roci_config, agent_config)?
};
```

Keep this as a compatibility shim only. Rich CLI create/open/import/export
commands remain owned by later `.6`.

- [ ] **Step 2: Build CLI**

Run:

```bash
cargo check -p roci-cli --features roci/lmstudio
```

Expected: pass.

- [ ] **Step 3: Run live tmux provider smoke**

Check local model server:

```bash
curl -sS http://127.0.0.1:1234/api/v0/models | head
```

If reachable, run a live prompt in tmux so user can attach:

```bash
tmux new-session -d -s roci-session-resume-live 'cargo run -p roci-cli --features roci/lmstudio -- chat --session-root /tmp/roci-session-resume-live --session-id live-resume-api --model lmstudio:local "reply with exactly: roci session resume live ok"'
tmux attach -t roci-session-resume-live
```

Expected: provider returns response, `/tmp/roci-session-resume-live/live-resume-api/metadata.json`,
`events.jsonl`, and `model_messages.jsonl` exist. Record exact output/status in
the task note. If LM Studio is not reachable, report that live proof is blocked
by missing local provider and keep automated payload proof as completed evidence.

---

## Task 11: Docs And Final Gates

**Files:**
- Modify: `docs/agent-runtime-chat.md`
- Modify: `docs/ARCHITECTURE.md`
- Test: command gates below.

- [ ] **Step 1: Update chat runtime docs**

In `docs/agent-runtime-chat.md`, add a durable resume section:

```markdown
### Durable session resume

`LocalSessionStore` owns local session create/open/import/export. Runtime
constructors do not create or open session files. Resume flows load
`SessionResumeState` from the store, then call `AgentRuntime::resume_session`.

`events.jsonl` is the canonical semantic runtime event log.
`runtime.snapshot.json` is cache only.
`model_messages.jsonl` is the canonical provider context ledger.
`model_messages.snapshot.json` is cache only.
```

- [ ] **Step 2: Update architecture docs**

In `docs/ARCHITECTURE.md`, add:

```markdown
- `session::LocalSessionStore` owns session filesystem lifecycle. Host apps
  choose the session root and call async store APIs before constructing or
  resuming an `AgentRuntime`.
- `AgentRuntime::{new,try_new}` consume prepared session configuration only.
  They do not write session metadata or create session directories.
```

- [ ] **Step 3: Run targeted gates**

Run:

```bash
cargo fmt --all -- --check
cargo test -p roci-core session::
cargo test -p roci-core --features agent "agent::runtime::tests::session"
cargo clippy -p roci-core --features agent -- -D warnings
```

Expected: pass.

- [ ] **Step 4: Run broader affected gates**

Run:

```bash
cargo test -p roci-core --features agent "agent::runtime::tests::chat_runtime"
cargo test -p roci-core --features agent "agent::runtime::tests::chat_projection"
cargo test -p roci-tools
cargo check -p roci-cli --features roci/lmstudio
```

Expected: pass.

- [ ] **Step 5: Record task note**

Run:

```bash
tsq --exact-id note add tsq-r0c1ses7.3 "Implementation completed for session resume APIs. Include final commands and live-proof status here."
```

Expected: note added. Do not close task until implementation and verification are complete.

---

## Known Execution Notes

- `ModelMessage::system`, `ModelMessage::user`, and `ModelMessage::assistant` exist in `crates/roci-core/src/types/message.rs`.
- Add `JsonlAgentRuntimeEventStore::all_events()` in Task 4; `LocalSessionStore` must not parse `events.jsonl` directly.
- `ChatProjector::apply_replayed_event` must apply existing `AgentRuntimeEventPayload` values without generating new events and must reuse current snapshot mutation helpers.
- Do not make `SessionSnapshot` import claim resource payload availability unless files exist under the new session root.
- Do not implement destructive replace in this task.
