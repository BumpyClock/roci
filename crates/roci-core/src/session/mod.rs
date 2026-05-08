//! Durable session identifiers, path contracts, and local file storage.

mod config;
mod error;
mod fs;
mod id;
mod metadata;
mod path;
mod resources;

pub use config::SessionConfig;
pub use error::{SessionError, SessionResult};
pub use fs::{LocalSessionFs, SessionDirEntry, SessionFileKind, SessionFileMetadata, SessionFs};
pub use id::SessionId;
pub use metadata::SessionMetadata;
pub use path::{LogicalPath, PathConventions, PathNamespace};
pub use resources::{LocalSessionResources, SessionResourceMetadata, SessionResourceNamespace};
