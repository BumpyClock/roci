use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::ModelMessage;

#[cfg(feature = "agent")]
use super::{
    locks::{ensure_store_root, SessionFileLock, SessionLockKind},
    SessionError, SessionId,
};
use super::{
    LogicalPath, SessionConfig, SessionMetadata, SessionModelPreferences, SessionResourceNamespace,
};

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
    /// Model settings persisted for subsequent turns in this session.
    pub model_preferences: SessionModelPreferences,
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
    #[cfg(feature = "agent")]
    _lock: SessionFileLock,
}

impl SessionLease {
    #[cfg(feature = "agent")]
    pub(crate) fn acquire(
        root: &std::path::Path,
        id: &SessionId,
    ) -> Result<Arc<Self>, SessionError> {
        let root = ensure_store_root(root)?;
        let session_path = root.join(id.as_str());
        let lock =
            SessionFileLock::try_acquire(&root, id, SessionLockKind::Runtime, &session_path)?;
        Ok(Arc::new(Self { _lock: lock }))
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
    #[cfg(feature = "agent")]
    pub(crate) fn new(
        session_config: SessionConfig,
        metadata: SessionMetadata,
        default_thread_id: ThreadId,
        runtime: RuntimeSnapshot,
    ) -> Self {
        let lease = SessionLease::acquire(&session_config.root, &session_config.id)
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
