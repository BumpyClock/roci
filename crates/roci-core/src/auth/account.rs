//! Account namespaces shared by login and provider execution.

use std::sync::Arc;

use super::{
    AuthError, ProviderCredentialRecord, ProviderCredentialStore, ProviderCredentialStoreError,
    Token, TokenRefreshLease, TokenStore,
};

/// A portable account name. Validation prevents file-name aliases and traversal.
pub fn validate_account(account: &str) -> Result<(), AuthError> {
    if account.is_empty()
        || account.len() > 64
        || !account.bytes().any(|b| b.is_ascii_alphanumeric())
        || !account
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(AuthError::InvalidResponse(
            "account names must be 1–64 lowercase letters, digits, hyphens".into(),
        ));
    }
    Ok(())
}

/// Bind all operations, including refresh transactions, to one named account.
pub(crate) struct AccountTokenStore {
    pub inner: Arc<dyn TokenStore>,
    pub account: String,
}

impl TokenStore for AccountTokenStore {
    fn unscoped_store(&self) -> Option<Arc<dyn TokenStore>> {
        Some(
            self.inner
                .unscoped_store()
                .unwrap_or_else(|| self.inner.clone()),
        )
    }
    fn refresh_coordination_identity(&self, _: &str) -> (usize, String) {
        self.inner.refresh_coordination_identity(&self.account)
    }

    fn load(&self, provider: &str, _: &str) -> Result<Option<Token>, AuthError> {
        self.inner.load(provider, &self.account)
    }
    fn save(&self, provider: &str, _: &str, token: &Token) -> Result<(), AuthError> {
        self.inner.save(provider, &self.account, token)
    }
    fn clear(&self, provider: &str, _: &str) -> Result<(), AuthError> {
        self.inner.clear(provider, &self.account)
    }
    fn save_if_current(
        &self,
        provider: &str,
        _: &str,
        expected: Option<&Token>,
        replacement: &Token,
    ) -> Result<bool, AuthError> {
        self.inner
            .save_if_current(provider, &self.account, expected, replacement)
    }
    fn try_acquire_refresh_lease(
        &self,
        provider: &str,
        _: &str,
    ) -> Result<Option<Box<dyn TokenRefreshLease>>, AuthError> {
        self.inner
            .try_acquire_refresh_lease(provider, &self.account)
    }
}

pub(crate) struct AccountCredentialStore {
    pub inner: Arc<dyn ProviderCredentialStore>,
    pub account: String,
}
impl AccountCredentialStore {
    fn key(&self, provider: &str) -> String {
        if self.account == "default" {
            provider.to_owned()
        } else {
            format!("{provider}@{}", self.account)
        }
    }
}
impl ProviderCredentialStore for AccountCredentialStore {
    fn unscoped_store(&self) -> Option<Arc<dyn ProviderCredentialStore>> {
        Some(
            self.inner
                .unscoped_store()
                .unwrap_or_else(|| self.inner.clone()),
        )
    }

    fn load(
        &self,
        provider: &str,
    ) -> Result<Option<ProviderCredentialRecord>, ProviderCredentialStoreError> {
        self.inner.load(&self.key(provider))
    }
    fn save(
        &self,
        provider: &str,
        record: &ProviderCredentialRecord,
    ) -> Result<(), ProviderCredentialStoreError> {
        self.inner.save(&self.key(provider), record)
    }
    fn clear(&self, provider: &str) -> Result<(), ProviderCredentialStoreError> {
        self.inner.clear(&self.key(provider))
    }
}
