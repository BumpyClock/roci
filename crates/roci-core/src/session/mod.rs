//! Durable session identifiers, path contracts, and local file storage.

#[cfg(feature = "agent")]
mod catalog;
mod config;
mod error;
mod fs;
mod id;
mod ledger;
mod locks;
mod metadata;
mod path;
#[cfg(feature = "agent")]
pub mod recovery;
mod resources;
mod snapshot;
#[cfg(feature = "agent")]
mod store;

#[cfg(feature = "agent")]
pub use catalog::{SessionArchiveFilter, SessionCatalogEntry, SessionCatalogQuery};
pub use config::SessionConfig;
pub use error::{SessionError, SessionResult};
pub use fs::{LocalSessionFs, SessionDirEntry, SessionFileKind, SessionFileMetadata, SessionFs};
pub use id::SessionId;
pub use ledger::{
    LocalProviderLedger, ProviderLedgerRecord, ProviderLedgerSnapshot, ProviderLedgerState,
};
pub use metadata::{SessionMetadata, SessionModelPreferences};
pub use path::{LogicalPath, PathConventions, PathNamespace};
pub use resources::{LocalSessionResources, SessionResourceMetadata, SessionResourceNamespace};
pub use snapshot::{
    AgentRuntimeEvent, CreateSessionOptions, ImportPolicy, ProviderLedgerSummary, RuntimeCursor,
    RuntimeSnapshot, RuntimeSnapshotCache, SessionLease, SessionResourceManifest,
    SessionResourceRef, SessionResumeState, SessionSnapshot, ThreadId,
};
#[cfg(feature = "agent")]
pub use store::LocalSessionStore;

#[cfg(feature = "agent")]
pub use recovery::{
    ProviderRecoveryReport, RecoveredSession, RecoveryReport, RecoverySeverity, RecoverySource,
    RecoverySourceReport, RecoverySourceStats, RecoverySourceStatus, RecoveryStats,
    RuntimeSnapshotCachePreview, SessionRecoverySource, RECOVERED_SESSION_ARTIFACT_TYPE,
};
