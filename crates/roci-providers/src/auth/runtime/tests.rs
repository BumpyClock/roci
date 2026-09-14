//! Behavioral coverage for refresh coordination and bounded request recovery.

use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use futures::FutureExt;
use roci_core::auth::{FileTokenStore, TokenRefreshLease, TokenStoreConfig};
use roci_core::types::{GenerationSettings, StreamEventType, Usage};
use tempfile::TempDir;
use tokio::sync::Notify;

fn token(access: &str, expired: bool) -> Token {
    Token {
        provider_metadata: None,
        access_token: access.into(),
        refresh_token: Some(format!("refresh-{access}")),
        id_token: None,
        expires_at: Some(Utc::now() + chrono::Duration::seconds(if expired { -1 } else { 3600 })),
        last_refresh: Some(Utc::now()),
        scopes: None,
        account_id: None,
    }
}

#[derive(Default)]
struct MemoryStore {
    value: Mutex<Option<Token>>,
    fail_save: AtomicBool,
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
}

struct MemoryRefreshLease {
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl TokenRefreshLease for MemoryRefreshLease {}

impl MemoryStore {
    fn seeded(value: Token) -> Arc<Self> {
        Arc::new(Self {
            value: Mutex::new(Some(value)),
            ..Self::default()
        })
    }
}

impl TokenStore for MemoryStore {
    fn load(&self, _provider: &str, _profile: &str) -> Result<Option<Token>, AuthError> {
        Ok(self.value.lock().unwrap().clone())
    }

    fn save(&self, _provider: &str, _profile: &str, token: &Token) -> Result<(), AuthError> {
        if self.fail_save.load(Ordering::SeqCst) {
            return Err(AuthError::Io("persistence unavailable".into()));
        }
        *self.value.lock().unwrap() = Some(token.clone());
        Ok(())
    }

    fn clear(&self, _provider: &str, _profile: &str) -> Result<(), AuthError> {
        *self.value.lock().unwrap() = None;
        Ok(())
    }

    fn save_if_current(
        &self,
        _provider: &str,
        _profile: &str,
        expected: Option<&Token>,
        replacement: &Token,
    ) -> Result<bool, AuthError> {
        let mut current = self.value.lock().unwrap();
        if current.as_ref() != expected {
            return Ok(false);
        }
        if self.fail_save.load(Ordering::SeqCst) {
            return Err(AuthError::Io("persistence unavailable".into()));
        }
        *current = Some(replacement.clone());
        Ok(true)
    }

    fn try_acquire_refresh_lease(
        &self,
        _provider: &str,
        _profile: &str,
    ) -> Result<Option<Box<dyn TokenRefreshLease>>, AuthError> {
        Ok(self
            .refresh_lock
            .clone()
            .try_lock_owned()
            .ok()
            .map(|guard| {
                Box::new(MemoryRefreshLease { _guard: guard }) as Box<dyn TokenRefreshLease>
            }))
    }
}

fn refresh_counter(calls: Arc<AtomicUsize>) -> Refresh {
    Arc::new(move |previous| {
        let calls = calls.clone();
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            assert!(previous.is_some());
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok(token("new", false))
        }
        .boxed()
    })
}

#[tokio::test]
async fn concurrent_sessions_refresh_rotating_token_once() {
    let store: Arc<dyn TokenStore> = MemoryStore::seeded(token("old", true));
    let calls = Arc::new(AtomicUsize::new(0));
    let refresh = refresh_counter(calls.clone());
    let sessions: Vec<_> = (0..12)
        .map(|_| OAuthSession::new(store.clone(), "provider", refresh.clone(), false))
        .collect();
    let results =
        futures::future::join_all(sessions.iter().map(|session| session.token(None))).await;
    assert!(results
        .into_iter()
        .all(|result| result.unwrap().access_token == "new"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        store
            .load("provider", "default")
            .unwrap()
            .unwrap()
            .refresh_token
            .as_deref(),
        Some("refresh-new")
    );
}

#[tokio::test]
async fn distinct_file_store_instances_share_refresh_lease() {
    let dir = TempDir::new().unwrap();
    let make_store = || {
        Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().into(),
        ))) as Arc<dyn TokenStore>
    };
    let first_store = make_store();
    first_store
        .save("provider", "default", &token("old", true))
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let refresh = refresh_counter(calls.clone());
    let first = OAuthSession::new(first_store, "provider", refresh.clone(), false);
    let second = OAuthSession::new(make_store(), "provider", refresh, false);
    let (first, second) = tokio::join!(first.token(None), second.token(None));
    assert_eq!(first.unwrap().access_token, "new");
    assert_eq!(second.unwrap().access_token, "new");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn refresh_persistence_failure_is_not_reported_as_success() {
    let old = token("old", true);
    let store = MemoryStore::seeded(old.clone());
    store.fail_save.store(true, Ordering::SeqCst);
    let calls = Arc::new(AtomicUsize::new(0));
    let session = OAuthSession::new(
        store.clone(),
        "provider",
        refresh_counter(calls.clone()),
        false,
    );
    assert!(matches!(session.token(None).await, Err(AuthError::Io(_))));
    assert_eq!(store.load("provider", "default").unwrap(), Some(old));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejected_token_already_replaced_does_not_refresh_again() {
    let store = MemoryStore::seeded(token("replacement", false));
    let calls = Arc::new(AtomicUsize::new(0));
    let session = OAuthSession::new(store, "provider", refresh_counter(calls.clone()), false);
    assert_eq!(
        session
            .token(Some("rejected-old"))
            .await
            .unwrap()
            .access_token,
        "replacement"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn logout_and_login_during_refresh_win_over_obsolete_result() {
    for login_again in [false, true] {
        let store = MemoryStore::seeded(token("old", true));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let refresh: Refresh = {
            let started = started.clone();
            let release = release.clone();
            Arc::new(move |_| {
                let started = started.clone();
                let release = release.clone();
                async move {
                    started.notify_one();
                    release.notified().await;
                    Ok(token("obsolete-refresh", false))
                }
                .boxed()
            })
        };
        let session = OAuthSession::new(store.clone(), "provider", refresh, false);
        let work = tokio::spawn(async move { session.token(None).await });
        started.notified().await;
        store.clear("provider", "default").unwrap();
        if login_again {
            store
                .save("provider", "default", &token("new-login", false))
                .unwrap();
        }
        release.notify_one();
        let result = work.await.unwrap();
        if login_again {
            assert_eq!(result.unwrap().access_token, "new-login");
            assert_eq!(
                store
                    .load("provider", "default")
                    .unwrap()
                    .unwrap()
                    .access_token,
                "new-login"
            );
        } else {
            assert!(matches!(result, Err(AuthError::NotLoggedIn)));
            assert!(store.load("provider", "default").unwrap().is_none());
        }
    }
}

#[derive(Clone)]
enum Event {
    Delta(StreamEventType),
    Error(u16),
}

enum Attempt {
    Success,
    Error(u16),
    Stream(Vec<Event>),
    TrackedStream(Vec<Event>, Arc<AtomicBool>),
}

struct TrackedStream {
    events: std::vec::IntoIter<Result<TextStreamDelta, RociError>>,
    dropped: Arc<AtomicBool>,
}

impl futures::Stream for TrackedStream {
    type Item = Result<TextStreamDelta, RociError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::task::Poll::Ready(self.events.next())
    }
}

impl Drop for TrackedStream {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

struct Script {
    attempts: Mutex<VecDeque<Attempt>>,
    used_tokens: Mutex<Vec<String>>,
}

struct ScriptedProvider {
    access: String,
    script: Arc<Script>,
    capabilities: ModelCapabilities,
}

fn api_error(status: u16) -> RociError {
    RociError::Api {
        status,
        message: "provider rejected request".into(),
        details: None,
        source: None,
    }
}

fn delta(event_type: StreamEventType) -> TextStreamDelta {
    TextStreamDelta {
        text: "visible".into(),
        event_type,
        tool_call: None,
        finish_reason: None,
        usage: None,
        reasoning: None,
        reasoning_signature: None,
        reasoning_type: None,
    }
}

impl ScriptedProvider {
    fn next(&self) -> Attempt {
        self.script
            .used_tokens
            .lock()
            .unwrap()
            .push(self.access.clone());
        self.script
            .attempts
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected request replay")
    }
}

#[async_trait]
impl ModelProvider for ScriptedProvider {
    fn provider_name(&self) -> &str {
        "test"
    }
    fn model_id(&self) -> &str {
        "test-model"
    }
    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        match self.next() {
            Attempt::Success => Ok(ProviderResponse {
                text: "ok".into(),
                usage: Usage::default(),
                tool_calls: vec![],
                finish_reason: None,
                thinking: vec![],
            }),
            Attempt::Error(status) => Err(api_error(status)),
            Attempt::Stream(_) | Attempt::TrackedStream(..) => {
                panic!("stream script used for generate_text")
            }
        }
    }

    async fn stream_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        let mut dropped = None;
        let events = match self.next() {
            Attempt::TrackedStream(events, flag) => {
                dropped = Some(flag);
                Attempt::Stream(events)
            }
            attempt => attempt,
        };
        let events = match events {
            Attempt::Success => vec![Ok(delta(StreamEventType::TextDelta))],
            Attempt::Error(status) => return Err(api_error(status)),
            Attempt::Stream(events) => events
                .into_iter()
                .map(|event| match event {
                    Event::Delta(kind) => Ok(delta(kind)),
                    Event::Error(status) => Err(api_error(status)),
                })
                .collect(),
            Attempt::TrackedStream(..) => unreachable!(),
        };
        if let Some(dropped) = dropped {
            return Ok(Box::pin(TrackedStream {
                events: events.into_iter(),
                dropped,
            }));
        }
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

fn managed(
    attempts: impl IntoIterator<Item = Attempt>,
) -> (Box<dyn ModelProvider>, Arc<Script>, Arc<AtomicUsize>) {
    let seed = token("old", false);
    let store = MemoryStore::seeded(seed.clone());
    let calls = Arc::new(AtomicUsize::new(0));
    let session = OAuthSession::new(store, "provider", refresh_counter(calls.clone()), false);
    let script = Arc::new(Script {
        attempts: Mutex::new(attempts.into_iter().collect()),
        used_tokens: Mutex::new(vec![]),
    });
    let build: Build = {
        let script = script.clone();
        Arc::new(move |token| {
            Ok(Box::new(ScriptedProvider {
                access: token.access_token.clone(),
                script: script.clone(),
                capabilities: ModelCapabilities::default(),
            }))
        })
    };
    (
        ManagedOAuthProvider::wrap(&seed, session, build).unwrap(),
        script,
        calls,
    )
}

fn request() -> ProviderRequest {
    ProviderRequest {
        messages: vec![],
        settings: GenerationSettings::default(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: Default::default(),
        metadata: Default::default(),
        payload_callback: None,
        session_id: None,
        transport: None,
    }
}

#[tokio::test]
async fn generate_retries_401_once_with_new_credential() {
    let (provider, script, refreshes) = managed([Attempt::Error(401), Attempt::Success]);
    assert_eq!(provider.generate_text(&request()).await.unwrap().text, "ok");
    assert_eq!(*script.used_tokens.lock().unwrap(), ["old", "new"]);
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn repeated_401_stops_after_one_recovery_and_403_never_refreshes() {
    for status in [401, 403] {
        let (provider, script, refreshes) =
            managed([Attempt::Error(status), Attempt::Error(status)]);
        assert!(
            matches!(provider.generate_text(&request()).await, Err(RociError::Api { status: actual, .. }) if actual == status)
        );
        assert_eq!(
            script.used_tokens.lock().unwrap().len(),
            if status == 401 { 2 } else { 1 }
        );
        assert_eq!(refreshes.load(Ordering::SeqCst), usize::from(status == 401));
    }
}

#[tokio::test]
async fn stream_retries_immediate_and_deferred_401_before_first_event() {
    for failure in [
        Attempt::Error(401),
        Attempt::Stream(vec![Event::Error(401)]),
    ] {
        let (provider, script, refreshes) = managed([failure, Attempt::Success]);
        let events: Vec<_> = provider
            .stream_text(&request())
            .await
            .unwrap()
            .collect()
            .await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].as_ref().unwrap().text, "visible");
        assert_eq!(*script.used_tokens.lock().unwrap(), ["old", "new"]);
        assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn stream_never_replays_after_any_event() {
    for kind in [
        StreamEventType::Start,
        StreamEventType::TextDelta,
        StreamEventType::ToolCallDelta,
    ] {
        let (provider, script, refreshes) =
            managed([Attempt::Stream(vec![Event::Delta(kind), Event::Error(401)])]);
        let events: Vec<_> = provider
            .stream_text(&request())
            .await
            .unwrap()
            .collect()
            .await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].as_ref().unwrap().event_type, kind);
        assert!(matches!(
            &events[1],
            Err(RociError::Api { status: 401, .. })
        ));
        assert_eq!(script.used_tokens.lock().unwrap().len(), 1);
        assert_eq!(refreshes.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn stream_repeated_401_is_bounded_and_403_is_not_retried() {
    for status in [401, 403] {
        let (provider, script, refreshes) = managed([
            Attempt::Stream(vec![Event::Error(status)]),
            Attempt::Stream(vec![Event::Error(status)]),
        ]);
        let events: Vec<_> = provider
            .stream_text(&request())
            .await
            .unwrap()
            .collect()
            .await;
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], Err(RociError::Api { status: actual, .. }) if *actual == status)
        );
        assert_eq!(
            script.used_tokens.lock().unwrap().len(),
            if status == 401 { 2 } else { 1 }
        );
        assert_eq!(refreshes.load(Ordering::SeqCst), usize::from(status == 401));
    }
}

#[tokio::test]
async fn rejected_stream_is_dropped_before_refresh_waits() {
    let dropped = Arc::new(AtomicBool::new(false));
    let seed = token("old", false);
    let store = MemoryStore::seeded(seed.clone());
    let refresh: Refresh = {
        let dropped = dropped.clone();
        Arc::new(move |_| {
            assert!(
                dropped.load(Ordering::SeqCst),
                "upstream stream retained during refresh"
            );
            async { Ok(token("new", false)) }.boxed()
        })
    };
    let script = Arc::new(Script {
        attempts: Mutex::new(VecDeque::from([
            Attempt::TrackedStream(vec![Event::Error(401)], dropped.clone()),
            Attempt::Success,
        ])),
        used_tokens: Mutex::new(vec![]),
    });
    let build: Build = Arc::new(move |token| {
        Ok(Box::new(ScriptedProvider {
            access: token.access_token.clone(),
            script: script.clone(),
            capabilities: ModelCapabilities::default(),
        }))
    });
    let provider = ManagedOAuthProvider::wrap(
        &seed,
        OAuthSession::new(store, "provider", refresh, false),
        build,
    )
    .unwrap();
    assert!(provider
        .stream_text(&request())
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .is_ok());
}

#[tokio::test]
async fn dropping_returned_stream_releases_upstream_immediately() {
    let dropped = Arc::new(AtomicBool::new(false));
    let (provider, _, _) = managed([Attempt::TrackedStream(
        vec![Event::Delta(StreamEventType::TextDelta)],
        dropped.clone(),
    )]);
    let stream = provider.stream_text(&request()).await.unwrap();
    assert!(!dropped.load(Ordering::SeqCst));
    drop(stream);
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn explicit_request_key_never_triggers_oauth_recovery() {
    for streaming in [false, true] {
        let (provider, script, refreshes) = managed([Attempt::Error(401)]);
        let mut request = request();
        request.api_key_override = Some("explicit-key".into());
        let error = if streaming {
            provider.stream_text(&request).await.err().unwrap()
        } else {
            provider.generate_text(&request).await.unwrap_err()
        };
        assert!(matches!(error, RociError::Api { status: 401, .. }));
        assert_eq!(script.used_tokens.lock().unwrap().len(), 1);
        assert_eq!(refreshes.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn missing_derived_token_is_exchanged_and_persisted() {
    let store = Arc::new(MemoryStore::default());
    let refresh: Refresh = Arc::new(|previous| {
        assert!(previous.is_none());
        async { Ok(token("derived", false)) }.boxed()
    });
    let session = OAuthSession::new(store.clone(), "derived", refresh, true);
    assert_eq!(session.token(None).await.unwrap().access_token, "derived");
    assert_eq!(
        store
            .load("derived", "default")
            .unwrap()
            .unwrap()
            .access_token,
        "derived"
    );
}

#[tokio::test]
async fn separately_scoped_configs_share_custom_store_refresh_coordination() {
    let raw: Arc<dyn TokenStore> = MemoryStore::seeded(token("old", true));
    let make_config = || {
        roci_core::config::RociConfig::new()
            .with_token_store(Some(raw.clone()))
            .with_provider_credential_store(None)
            .with_account("work")
            .unwrap()
    };
    let first_config = make_config();
    let second_config = make_config();
    assert!(!Arc::ptr_eq(
        first_config.token_store().unwrap(),
        second_config.token_store().unwrap()
    ));
    let calls = Arc::new(AtomicUsize::new(0));
    let refresh = refresh_counter(calls.clone());
    let first = OAuthSession::new(
        first_config.token_store().unwrap().clone(),
        "provider",
        refresh.clone(),
        false,
    );
    let second = OAuthSession::new(
        second_config.token_store().unwrap().clone(),
        "provider",
        refresh,
        false,
    );
    let (first, second) = tokio::join!(first.token(None), second.token(None));
    assert_eq!(first.unwrap().access_token, "new");
    assert_eq!(second.unwrap().access_token, "new");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
