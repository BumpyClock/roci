//! OAuth device-code flows, token storage, and generic auth orchestration.

pub mod backend;
pub mod credential;
pub mod descriptor;
pub mod device_code;
pub mod error;
#[cfg(unix)]
mod file_credential;
#[cfg(all(test, unix))]
mod file_credential_tests;
pub mod host;
pub mod manager;
mod pending;
pub mod service;
pub mod status;
pub mod store;
pub mod token;

pub use backend::AuthBackend;
pub use credential::{
    InMemoryProviderCredentialStore, OsProviderCredentialStore, ProviderApiKey,
    ProviderCredentialRecord, ProviderCredentialStore, ProviderCredentialStoreError,
    ProviderEndpoint,
};
pub use descriptor::{CredentialFlow, ProviderDescriptor};
pub use device_code::DeviceCodeSession;
pub use error::AuthError;
#[cfg(unix)]
pub use file_credential::FileProviderCredentialStore;
pub use host::{HostAuthCompletion, HostAuthPollResult, HostAuthStep, LoginSessionId};
pub use manager::ProviderAuthManager;
pub use service::{AuthPollResult, AuthService, AuthStep};
pub use status::{ConfiguredSource, ProviderAuthState, ProviderAuthStatus};
pub use store::{FileTokenStore, TokenStore, TokenStoreConfig};
pub use token::Token;
