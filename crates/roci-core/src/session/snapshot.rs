use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::ModelMessage;

use super::{LogicalPath, SessionConfig, SessionError, SessionMetadata, SessionResourceNamespace};

#[cfg(feature = "agent")]
pub use crate::agent::runtime::chat::{
    AgentRuntimeEvent, RuntimeCursor, RuntimeSnapshot, ThreadId,
};

#[cfg(not(feature = "agent"))]
pub use fallback_runtime_types::{AgentRuntimeEvent, RuntimeCursor, RuntimeSnapshot, ThreadId};

#[cfg(not(feature = "agent"))]
mod fallback_runtime_types {
    use serde::{Deserialize, Serialize};
    use uuid::Uuid;

    /// Stable semantic thread id used when the `agent` feature is disabled.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(transparent)]
    pub struct ThreadId(Uuid);

    impl ThreadId {
        #[must_use]
        pub fn new() -> Self {
            Self(Uuid::new_v4())
        }

        #[must_use]
        pub const fn nil() -> Self {
            Self(Uuid::nil())
        }
    }

    impl Default for ThreadId {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Cursor into one thread's semantic event stream.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct RuntimeCursor {
        /// Thread this cursor belongs to.
        pub thread_id: ThreadId,
        /// Last observed event sequence for the thread.
        pub seq: u64,
    }

    /// Runtime snapshot placeholder used when chat runtime types are unavailable.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct RuntimeSnapshot {
        /// Snapshot schema version.
        pub schema_version: u16,
        /// Opaque thread snapshots.
        pub threads: Vec<serde_json::Value>,
    }

    /// Opaque semantic event value used when chat runtime types are unavailable.
    pub type AgentRuntimeEvent = serde_json::Value;
}

/// Options for creating a durable local session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CreateSessionOptions {
    pub id: Option<super::SessionId>,
    pub title: Option<String>,
    pub host_cwd: Option<PathBuf>,
    pub import_source: Option<PathBuf>,
    pub default_thread_id: Option<ThreadId>,
}

/// Session snapshot import policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportPolicy {
    FailIfExists,
    NewId(Option<super::SessionId>),
}

/// Portable session snapshot manifest.
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

/// Provider ledger summary for snapshot/export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderLedgerSummary {
    pub thread_id: ThreadId,
    pub latest_seq: u64,
    pub effective_history: Vec<ModelMessage>,
}

/// Manifest of session resource refs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SessionResourceManifest {
    pub plan: Option<SessionResourceRef>,
    pub workspace: Option<SessionResourceRef>,
    pub artifacts: Vec<SessionResourceRef>,
    pub temp_files: Vec<SessionResourceRef>,
    pub checkpoints: Vec<SessionResourceRef>,
    pub files: Vec<SessionResourceRef>,
}

/// Snapshot reference to a resource, without file bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionResourceRef {
    pub namespace: SessionResourceNamespace,
    pub logical_path: Option<LogicalPath>,
    pub storage_path: PathBuf,
    pub len: u64,
    pub updated_at: Option<DateTime<Utc>>,
    pub available: bool,
}

/// Runtime snapshot cache materialized from canonical events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSnapshotCache {
    pub schema_version: u16,
    pub default_thread_id: ThreadId,
    pub runtime: RuntimeSnapshot,
    pub latest_cursors: Vec<RuntimeCursor>,
    pub generated_at: DateTime<Utc>,
}

/// Placeholder session write lease for resume state ownership.
#[derive(Debug)]
pub struct SessionLease {
    key: PathBuf,
}

impl SessionLease {
    pub(crate) fn acquire(key: PathBuf) -> Result<Arc<Self>, SessionError> {
        let key = canonical_lease_key(key)?;
        let leases = ACTIVE_SESSION_LEASES.get_or_init(Default::default);
        let mut guard = leases
            .lock()
            .expect("session lease registry mutex poisoned");
        if !guard.insert(key.clone()) {
            return Err(SessionError::AlreadyOpen { path: key });
        }
        Ok(Arc::new(Self { key }))
    }
}

static ACTIVE_SESSION_LEASES: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

fn canonical_lease_key(key: PathBuf) -> Result<PathBuf, SessionError> {
    if key.exists() {
        return std::fs::canonicalize(&key).map_err(|source| SessionError::io(&key, source));
    }
    let Some(parent) = key.parent() else {
        return Ok(key);
    };
    let canonical_parent =
        std::fs::canonicalize(parent).map_err(|source| SessionError::io(parent, source))?;
    Ok(canonical_parent.join(
        key.file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("")),
    ))
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

/// Prepared local state used to resume a runtime.
#[allow(dead_code)]
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
    pub(crate) lease: Arc<SessionLease>,
}

#[allow(dead_code)]
impl SessionResumeState {
    #[must_use]
    pub(crate) fn new(
        session_config: SessionConfig,
        metadata: SessionMetadata,
        default_thread_id: ThreadId,
        runtime: RuntimeSnapshot,
    ) -> Self {
        let lease = SessionLease::acquire(session_config.conventions().root().to_path_buf())
            .expect("new resume state should acquire session lease");
        Self {
            session_config,
            metadata,
            default_thread_id,
            runtime,
            model_messages: Vec::new(),
            resources: SessionResourceManifest::default(),
            events: Vec::new(),
            event_cursors: Vec::new(),
            provider_ledger_seq: 0,
            lease,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionId, SessionResourceNamespace};

    #[test]
    fn session_snapshot_serializes_manifest_without_resource_bytes() {
        let session_id = SessionId::parse("snapshot-session").expect("session id");
        let thread_id = ThreadId::new();
        let metadata = SessionMetadata::new(session_id, None, None);
        let snapshot = SessionSnapshot {
            schema_version: 1,
            metadata,
            default_thread_id: thread_id,
            runtime: RuntimeSnapshot {
                schema_version: 1,
                threads: Vec::new(),
            },
            events: Vec::new(),
            provider_ledger: ProviderLedgerSummary {
                thread_id,
                latest_seq: 0,
                effective_history: Vec::new(),
            },
            resources: SessionResourceManifest {
                files: vec![SessionResourceRef {
                    namespace: SessionResourceNamespace::Files,
                    logical_path: Some(LogicalPath::parse("notes.txt").expect("logical path")),
                    storage_path: PathBuf::from("files/notes.txt"),
                    len: 12,
                    updated_at: None,
                    available: true,
                }],
                ..SessionResourceManifest::default()
            },
            exported_at: Utc::now(),
        };

        let json = serde_json::to_string(&snapshot).expect("serialize snapshot");

        assert!(json.contains("files/notes.txt"));
        assert!(json.contains("\"len\":12"));
        assert!(!json.contains("hello world"));
        assert!(!json.contains("bytes"));
        assert!(!json.contains("payload"));
    }
}
