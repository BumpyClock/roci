//! Provider API-key credential persistence.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const CREDENTIAL_RECORD_VERSION: u32 = 1;
const KEYRING_SERVICE: &str = "dev.roci.provider-credentials";

/// Secret API-key input accepted by provider configuration APIs.
///
/// This type deliberately implements neither `Display` nor `Serialize`; its
/// `Debug` representation is always redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderApiKey(String);

impl ProviderApiKey {
    /// Wrap API-key material supplied by a trusted host input.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the secret for provider transport or protected persistence.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ProviderApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProviderApiKey([REDACTED])")
    }
}

/// Provider endpoint stored alongside an API key.
///
/// Endpoints are transport configuration, not credentials, but `Debug` stays
/// redacted because URLs can contain user-info or query secrets.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderEndpoint(String);

impl ProviderEndpoint {
    /// Wrap a provider endpoint supplied by a trusted host input.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the endpoint string for provider transport configuration.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ProviderEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProviderEndpoint([REDACTED])")
    }
}

/// Versioned provider API-key record stored under a canonical provider key.
///
/// Role: carry one Roci-owned API key and its optional endpoint between
/// [`ProviderCredentialStore`] implementations and [`crate::config::RociConfig`].
/// Construct records in hosts, persist them through a store, and let config
/// resolve them below explicit/environment values. `Debug` always redacts the
/// API key and endpoint, and this type intentionally does not implement
/// `Serialize`.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderCredentialRecord {
    /// Provider API-key secret.
    pub api_key: ProviderApiKey,
    /// Optional provider endpoint/base URL.
    pub endpoint: Option<ProviderEndpoint>,
}

impl ProviderCredentialRecord {
    /// Build the current-version credential record.
    pub fn new(api_key: ProviderApiKey, endpoint: Option<ProviderEndpoint>) -> Self {
        Self { api_key, endpoint }
    }
}

impl fmt::Debug for ProviderCredentialRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderCredentialRecord")
            .field("api_key", &"[REDACTED]")
            .field("endpoint", &self.endpoint.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Secret-safe provider credential persistence failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProviderCredentialStoreError {
    /// Protected operating-system credential storage cannot be accessed.
    #[error("protected credential store is unavailable")]
    Unavailable,
    /// Stored data cannot be encoded or decoded safely.
    #[error("provider credential record is invalid")]
    InvalidRecord,
    /// Stored data uses a record version this Roci build does not understand.
    #[error("unsupported provider credential record version {0}")]
    UnsupportedVersion(u32),
}

/// Protected persistence for Roci-owned provider API-key records.
///
/// Role: load, atomically replace, and clear one record per canonical provider.
/// Production hosts should use [`OsProviderCredentialStore`], which delegates
/// to the OS credential manager and fails closed. Tests and embedding hosts can
/// inject [`InMemoryProviderCredentialStore`] through
/// [`crate::config::RociConfig::with_provider_credential_store`].
pub trait ProviderCredentialStore: Send + Sync {
    /// Load the provider record, or `None` when no Roci-owned record exists.
    fn load(
        &self,
        provider: &str,
    ) -> Result<Option<ProviderCredentialRecord>, ProviderCredentialStoreError>;

    /// Atomically replace the provider record in protected storage.
    fn save(
        &self,
        provider: &str,
        record: &ProviderCredentialRecord,
    ) -> Result<(), ProviderCredentialStoreError>;

    /// Clear the provider record. Missing records are treated as success.
    fn clear(&self, provider: &str) -> Result<(), ProviderCredentialStoreError>;
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredProviderCredentialRecord {
    version: u32,
    api_key: String,
    endpoint: Option<String>,
}

impl StoredProviderCredentialRecord {
    pub(crate) fn from_record(record: &ProviderCredentialRecord) -> Self {
        Self {
            version: CREDENTIAL_RECORD_VERSION,
            api_key: record.api_key.expose_secret().to_string(),
            endpoint: record
                .endpoint
                .as_ref()
                .map(|endpoint| endpoint.as_str().to_string()),
        }
    }

    pub(crate) fn validate_version(&self) -> Result<(), ProviderCredentialStoreError> {
        if self.version == CREDENTIAL_RECORD_VERSION {
            Ok(())
        } else {
            Err(ProviderCredentialStoreError::UnsupportedVersion(
                self.version,
            ))
        }
    }

    pub(crate) fn into_record(
        self,
    ) -> Result<ProviderCredentialRecord, ProviderCredentialStoreError> {
        self.validate_version()?;
        Ok(ProviderCredentialRecord::new(
            ProviderApiKey::new(self.api_key),
            self.endpoint.map(ProviderEndpoint::new),
        ))
    }
}

trait CredentialBackend: Send + Sync {
    fn get(&self, provider: &str) -> Result<Option<String>, ProviderCredentialStoreError>;
    fn set(&self, provider: &str, value: &str) -> Result<(), ProviderCredentialStoreError>;
    fn delete(&self, provider: &str) -> Result<(), ProviderCredentialStoreError>;
}

struct KeyringBackend;

impl KeyringBackend {
    fn entry(provider: &str) -> Result<keyring::Entry, ProviderCredentialStoreError> {
        keyring::Entry::new(KEYRING_SERVICE, provider)
            .map_err(|_| ProviderCredentialStoreError::Unavailable)
    }
}

impl CredentialBackend for KeyringBackend {
    fn get(&self, provider: &str) -> Result<Option<String>, ProviderCredentialStoreError> {
        match Self::entry(provider)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(ProviderCredentialStoreError::Unavailable),
        }
    }

    fn set(&self, provider: &str, value: &str) -> Result<(), ProviderCredentialStoreError> {
        Self::entry(provider)?
            .set_password(value)
            .map_err(|_| ProviderCredentialStoreError::Unavailable)
    }

    fn delete(&self, provider: &str) -> Result<(), ProviderCredentialStoreError> {
        match Self::entry(provider)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(ProviderCredentialStoreError::Unavailable),
        }
    }
}

/// Production provider credential store backed only by the OS credential manager.
///
/// Uses Keychain Services on macOS, Windows Credential Manager on Windows, and
/// Secret Service on Linux. It never falls back to plaintext storage.
pub struct OsProviderCredentialStore {
    backend: Arc<dyn CredentialBackend>,
}

impl OsProviderCredentialStore {
    /// Create an OS-backed credential store.
    pub fn new() -> Self {
        Self {
            backend: Arc::new(KeyringBackend),
        }
    }

    #[cfg(test)]
    fn with_backend(backend: Arc<dyn CredentialBackend>) -> Self {
        Self { backend }
    }
}

impl Default for OsProviderCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for OsProviderCredentialStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OsProviderCredentialStore(..)")
    }
}

impl ProviderCredentialStore for OsProviderCredentialStore {
    fn load(
        &self,
        provider: &str,
    ) -> Result<Option<ProviderCredentialRecord>, ProviderCredentialStoreError> {
        let Some(raw) = self.backend.get(provider)? else {
            return Ok(None);
        };
        let stored: StoredProviderCredentialRecord =
            serde_json::from_str(&raw).map_err(|_| ProviderCredentialStoreError::InvalidRecord)?;
        stored.into_record().map(Some)
    }

    fn save(
        &self,
        provider: &str,
        record: &ProviderCredentialRecord,
    ) -> Result<(), ProviderCredentialStoreError> {
        let raw = serde_json::to_string(&StoredProviderCredentialRecord::from_record(record))
            .map_err(|_| ProviderCredentialStoreError::InvalidRecord)?;
        self.backend.set(provider, &raw)
    }

    fn clear(&self, provider: &str) -> Result<(), ProviderCredentialStoreError> {
        self.backend.delete(provider)
    }
}

/// Hermetic in-memory provider credential store for tests and embedding hosts.
///
/// Records live only for this value's lifetime and never touch the OS
/// credential manager. Clones share the same in-memory records.
#[derive(Clone, Default)]
pub struct InMemoryProviderCredentialStore {
    records: Arc<RwLock<HashMap<String, ProviderCredentialRecord>>>,
}

impl fmt::Debug for InMemoryProviderCredentialStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("InMemoryProviderCredentialStore([REDACTED])")
    }
}

impl ProviderCredentialStore for InMemoryProviderCredentialStore {
    fn load(
        &self,
        provider: &str,
    ) -> Result<Option<ProviderCredentialRecord>, ProviderCredentialStoreError> {
        Ok(self
            .records
            .read()
            .map_err(|_| ProviderCredentialStoreError::Unavailable)?
            .get(provider)
            .cloned())
    }

    fn save(
        &self,
        provider: &str,
        record: &ProviderCredentialRecord,
    ) -> Result<(), ProviderCredentialStoreError> {
        self.records
            .write()
            .map_err(|_| ProviderCredentialStoreError::Unavailable)?
            .insert(provider.to_string(), record.clone());
        Ok(())
    }

    fn clear(&self, provider: &str) -> Result<(), ProviderCredentialStoreError> {
        self.records
            .write()
            .map_err(|_| ProviderCredentialStoreError::Unavailable)?
            .remove(provider);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeBackend {
        value: Mutex<Option<String>>,
        unavailable: bool,
    }

    impl CredentialBackend for FakeBackend {
        fn get(&self, _provider: &str) -> Result<Option<String>, ProviderCredentialStoreError> {
            if self.unavailable {
                Err(ProviderCredentialStoreError::Unavailable)
            } else {
                Ok(self.value.lock().unwrap().clone())
            }
        }

        fn set(&self, _provider: &str, value: &str) -> Result<(), ProviderCredentialStoreError> {
            if self.unavailable {
                Err(ProviderCredentialStoreError::Unavailable)
            } else {
                *self.value.lock().unwrap() = Some(value.to_string());
                Ok(())
            }
        }

        fn delete(&self, _provider: &str) -> Result<(), ProviderCredentialStoreError> {
            if self.unavailable {
                Err(ProviderCredentialStoreError::Unavailable)
            } else {
                *self.value.lock().unwrap() = None;
                Ok(())
            }
        }
    }

    fn sample_record() -> ProviderCredentialRecord {
        ProviderCredentialRecord::new(
            ProviderApiKey::new("sk-secret"),
            Some(ProviderEndpoint::new("https://api.example/v1")),
        )
    }

    #[test]
    fn os_store_round_trips_versioned_json_through_fake_backend() {
        let backend = Arc::new(FakeBackend::default());
        let store = OsProviderCredentialStore::with_backend(backend.clone());
        let expected = sample_record();

        store.save("example", &expected).unwrap();
        let raw = backend.value.lock().unwrap().clone().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&raw).unwrap()["version"],
            1
        );
        assert_eq!(store.load("example").unwrap(), Some(expected));
        store.clear("example").unwrap();
        assert_eq!(store.load("example").unwrap(), None);
    }

    #[test]
    fn unsupported_record_version_is_rejected() {
        let backend = Arc::new(FakeBackend::default());
        *backend.value.lock().unwrap() =
            Some(r#"{"version":2,"api_key":"hidden","endpoint":null}"#.to_string());
        let store = OsProviderCredentialStore::with_backend(backend);

        assert_eq!(
            store.load("example"),
            Err(ProviderCredentialStoreError::UnsupportedVersion(2))
        );
    }

    #[test]
    fn backend_errors_are_mapped_without_detail_or_secret() {
        let store = OsProviderCredentialStore::with_backend(Arc::new(FakeBackend {
            value: Mutex::new(None),
            unavailable: true,
        }));

        let error = store.save("example", &sample_record()).unwrap_err();
        assert_eq!(error, ProviderCredentialStoreError::Unavailable);
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("sk-secret"));
        assert!(!rendered.contains("api.example"));
    }

    #[test]
    fn record_debug_redacts_all_values() {
        let debug = format!("{:?}", sample_record());
        assert!(!debug.contains("sk-secret"));
        assert!(!debug.contains("api.example"));
    }

    #[test]
    fn memory_store_round_trip_and_clear() {
        let store = InMemoryProviderCredentialStore::default();
        let expected = sample_record();
        store.save("example", &expected).unwrap();
        assert_eq!(store.load("example").unwrap(), Some(expected));
        store.clear("example").unwrap();
        assert_eq!(store.load("example").unwrap(), None);
    }
}
