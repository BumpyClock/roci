//! Durable session identifiers, path contracts, and local file storage.

mod config;
mod error;
mod fs;
mod id;
mod ledger;
mod metadata;
mod path;
mod resources;
mod snapshot;
#[cfg(feature = "agent")]
mod store;

pub use config::SessionConfig;
pub use error::{SessionError, SessionResult};
pub use fs::{LocalSessionFs, SessionDirEntry, SessionFileKind, SessionFileMetadata, SessionFs};
pub use id::SessionId;
pub use ledger::{
    LocalProviderLedger, ProviderLedgerRecord, ProviderLedgerSnapshot, ProviderLedgerState,
};
pub use metadata::SessionMetadata;
pub use path::{LogicalPath, PathConventions, PathNamespace};
pub use resources::{LocalSessionResources, SessionResourceMetadata, SessionResourceNamespace};
pub use snapshot::{
    AgentRuntimeEvent, CreateSessionOptions, ImportPolicy, ProviderLedgerSummary, RuntimeCursor,
    RuntimeSnapshot, RuntimeSnapshotCache, SessionLease, SessionResourceManifest,
    SessionResourceRef, SessionResumeState, SessionSnapshot, ThreadId,
};
#[cfg(feature = "agent")]
pub use store::LocalSessionStore;
