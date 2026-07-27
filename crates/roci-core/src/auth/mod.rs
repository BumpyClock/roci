//! OAuth device-code flows, token storage, and generic auth orchestration.

pub mod backend;
pub mod descriptor;
pub mod device_code;
pub mod error;
pub mod host;
pub mod manager;
pub mod service;
pub mod status;
pub mod store;
pub mod token;

pub use backend::AuthBackend;
pub use descriptor::{CredentialFlow, ProviderDescriptor};
pub use device_code::DeviceCodeSession;
pub use error::AuthError;
pub use host::{HostAuthCompletion, HostAuthPollResult, HostAuthStep, LoginSessionId};
pub use manager::ProviderAuthManager;
pub use service::{AuthPollResult, AuthService, AuthStep};
pub use status::{ConfiguredSource, ProviderAuthState, ProviderAuthStatus};
pub use store::{FileTokenStore, TokenStore, TokenStoreConfig};
pub use token::Token;
