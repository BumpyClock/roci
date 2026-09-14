//! Request-time OAuth acquisition and bounded recovery shared by native providers.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

#[cfg(any(
    test,
    feature = "openai",
    feature = "anthropic",
    feature = "google",
    feature = "cursor"
))]
use async_trait::async_trait;
use chrono::Utc;
use futures::future::BoxFuture;
#[cfg(any(
    test,
    feature = "openai",
    feature = "anthropic",
    feature = "google",
    feature = "cursor"
))]
use futures::{stream::BoxStream, StreamExt};
use roci_core::auth::{AuthError, Token, TokenStore};
#[cfg(any(
    test,
    feature = "openai",
    feature = "anthropic",
    feature = "google",
    feature = "cursor"
))]
use roci_core::context::overflow::OverflowSignal;
#[cfg(any(
    test,
    feature = "openai",
    feature = "anthropic",
    feature = "google",
    feature = "cursor"
))]
use roci_core::error::RociError;
#[cfg(any(
    test,
    feature = "openai",
    feature = "anthropic",
    feature = "google",
    feature = "cursor"
))]
use roci_core::models::capabilities::ModelCapabilities;
#[cfg(any(
    test,
    feature = "openai",
    feature = "anthropic",
    feature = "google",
    feature = "cursor"
))]
use roci_core::provider::{ModelProvider, ProviderRequest, ProviderResponse};
#[cfg(any(
    test,
    feature = "openai",
    feature = "anthropic",
    feature = "google",
    feature = "cursor"
))]
use roci_core::types::TextStreamDelta;

pub(crate) type Refresh =
    Arc<dyn Fn(Option<Token>) -> BoxFuture<'static, Result<Token, AuthError>> + Send + Sync>;
#[cfg(any(
    test,
    feature = "openai",
    feature = "anthropic",
    feature = "google",
    feature = "cursor"
))]
pub(crate) type Build =
    Arc<dyn Fn(&Token) -> Result<Box<dyn ModelProvider>, RociError> + Send + Sync>;
type RefreshLocks = HashMap<(usize, String, String), Weak<tokio::sync::Mutex<()>>>;

fn shared_refresh_lock(
    store: &Arc<dyn TokenStore>,
    key: &str,
    profile: &str,
) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<RefreshLocks>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    let (backing_store, account) = store.refresh_coordination_identity(profile);
    let identity = (backing_store, account, key.to_string());
    if let Some(lock) = locks.get(&identity).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(identity, Arc::downgrade(&lock));
    lock
}

pub(crate) struct OAuthSession {
    store: Arc<dyn TokenStore>,
    store_key: String,
    profile: String,
    refresh: Refresh,
    lock: Arc<tokio::sync::Mutex<()>>,
    /// Derived tokens (Copilot) can be absent while their primary credential still exists.
    derived: bool,
}

impl OAuthSession {
    pub(crate) fn new(
        store: Arc<dyn TokenStore>,
        store_key: &str,
        refresh: Refresh,
        derived: bool,
    ) -> Self {
        let lock = shared_refresh_lock(&store, store_key, "default");
        Self {
            store,
            store_key: store_key.into(),
            profile: "default".into(),
            refresh,
            lock,
            derived,
        }
    }

    pub(crate) fn with_profile(mut self, profile: &str) -> Self {
        self.profile = profile.into();
        self.lock = shared_refresh_lock(&self.store, &self.store_key, profile);
        self
    }

    pub(crate) async fn token(&self, rejected: Option<&str>) -> Result<Token, AuthError> {
        // Only this credential is serialized. No blocking mutex is held over network I/O.
        let _local = tokio::time::timeout(Duration::from_secs(60), self.lock.lock())
            .await
            .map_err(|_| AuthError::Network("credential refresh is busy; retry shortly".into()))?;
        let acquire = async {
            loop {
                if let Some(lease) = self
                    .store
                    .try_acquire_refresh_lease(&self.store_key, &self.profile)?
                {
                    return Ok::<_, AuthError>(lease);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        let _lease = tokio::time::timeout(Duration::from_secs(60), acquire)
            .await
            .map_err(|_| {
                AuthError::Network("credential refresh is busy; retry shortly".into())
            })??;
        let current = self.store.load(&self.store_key, &self.profile)?;
        if let Some(token) = current.as_ref() {
            let replaced = rejected.is_some_and(|failed| failed != token.access_token);
            if replaced || (rejected.is_none() && !needs_refresh(token)) {
                return Ok(token.clone());
            }
            if !self.derived
                && token
                    .refresh_token
                    .as_ref()
                    .is_none_or(|token| token.is_empty())
            {
                return Err(AuthError::ExpiredOrInvalidGrant);
            }
        } else if !self.derived {
            return Err(AuthError::NotLoggedIn);
        }
        let refreshed =
            tokio::time::timeout(Duration::from_secs(45), (self.refresh)(current.clone()))
                .await
                .map_err(|_| AuthError::Network("credential refresh timed out".into()))??;
        if refreshed.access_token.trim().is_empty() {
            return Err(AuthError::InvalidResponse(
                "refresh returned an empty access token".into(),
            ));
        }
        if self.store.save_if_current(
            &self.store_key,
            &self.profile,
            current.as_ref(),
            &refreshed,
        )? {
            Ok(refreshed)
        } else {
            // Logout or a new login won while the refresh was in flight.
            self.store
                .load(&self.store_key, &self.profile)?
                .ok_or(AuthError::NotLoggedIn)
        }
    }
}

fn needs_refresh(token: &Token) -> bool {
    if token.access_token.is_empty() {
        return true;
    }
    if let Some(expiry) = token.expires_at {
        return expiry <= Utc::now() + chrono::Duration::minutes(5);
    }
    token.refresh_token.is_some()
        && token
            .last_refresh
            .is_none_or(|last| last <= Utc::now() - chrono::Duration::days(8))
}

#[cfg(any(
    test,
    feature = "openai",
    feature = "anthropic",
    feature = "google",
    feature = "cursor"
))]
pub(crate) struct ManagedOAuthProvider {
    baseline: Box<dyn ModelProvider>,
    session: OAuthSession,
    build: Build,
}

#[cfg(any(
    test,
    feature = "openai",
    feature = "anthropic",
    feature = "google",
    feature = "cursor"
))]
impl ManagedOAuthProvider {
    pub(crate) fn wrap(
        seed: &Token,
        session: OAuthSession,
        build: Build,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        Ok(Box::new(Self {
            baseline: build(seed)?,
            session,
            build,
        }))
    }
}

#[cfg(any(
    test,
    feature = "openai",
    feature = "anthropic",
    feature = "google",
    feature = "cursor"
))]
fn unauthorized(error: &RociError) -> bool {
    matches!(error, RociError::Api { status: 401, .. })
}

#[cfg(any(
    test,
    feature = "openai",
    feature = "anthropic",
    feature = "google",
    feature = "cursor"
))]
#[async_trait]
impl ModelProvider for ManagedOAuthProvider {
    fn provider_name(&self) -> &str {
        self.baseline.provider_name()
    }
    fn model_id(&self) -> &str {
        self.baseline.model_id()
    }
    fn capabilities(&self) -> &ModelCapabilities {
        self.baseline.capabilities()
    }
    fn classify_overflow(&self, error: &RociError) -> Option<OverflowSignal> {
        self.baseline.classify_overflow(error)
    }

    async fn generate_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        if request.api_key_override.is_some() {
            return self.baseline.generate_text(request).await;
        }
        let token = self.session.token(None).await?;
        let result = (self.build)(&token)?.generate_text(request).await;
        match result {
            Err(error) if unauthorized(&error) => {
                let refreshed = self.session.token(Some(&token.access_token)).await?;
                (self.build)(&refreshed)?.generate_text(request).await
            }
            other => other,
        }
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        if request.api_key_override.is_some() {
            return self.baseline.stream_text(request).await;
        }
        let mut token = self.session.token(None).await?;
        // Some transports defer HTTP errors until the first stream poll. Retry only
        // before delivering any event, so tool calls and visible text are never replayed.
        for attempt in 0..2 {
            let opened = (self.build)(&token)?.stream_text(request).await;
            let (mut stream, first) = match opened {
                Ok(mut stream) => {
                    let first = stream.next().await;
                    (Some(stream), first)
                }
                Err(error) => (None, Some(Err(error))),
            };
            if let Some(Err(ref error)) = first {
                if attempt == 0 && unauthorized(error) {
                    // Release transport tasks, upload senders, and durable session
                    // leases before waiting on an unrelated credential refresh.
                    drop(stream.take());
                    token = self.session.token(Some(&token.access_token)).await?;
                    continue;
                }
            }
            return match stream.take() {
                Some(stream) => Ok(Box::pin(futures::stream::iter(first).chain(stream))),
                None => Err(first
                    .expect("failed open has an error")
                    .expect_err("failed open has an error")),
            };
        }
        unreachable!("bounded auth retry returns on its final attempt")
    }
}

#[cfg(test)]
mod tests;
