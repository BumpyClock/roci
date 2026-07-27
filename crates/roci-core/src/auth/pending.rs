//! Bounded, expiring storage for secret pending-login material.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};

use super::device_code::DeviceCodeSession;
use super::error::AuthError;
use super::host::LoginSessionId;

const DEFAULT_PENDING_LOGIN_CAPACITY: usize = 32;
const PKCE_SESSION_LIFETIME_MINUTES: i64 = 10;

/// Internal pending login material that must never cross the manager boundary.
#[derive(Clone)]
pub(crate) enum PendingLogin {
    DeviceCode {
        provider_alias: String,
        canonical: String,
        session: DeviceCodeSession,
    },
    Pkce {
        provider_alias: String,
        canonical: String,
        state: String,
        session_data: serde_json::Value,
        expires_at: DateTime<Utc>,
    },
}

impl PendingLogin {
    pub(crate) fn pkce(
        provider_alias: String,
        canonical: String,
        state: String,
        session_data: serde_json::Value,
    ) -> Self {
        Self::Pkce {
            provider_alias,
            canonical,
            state,
            session_data,
            expires_at: Utc::now() + Duration::minutes(PKCE_SESSION_LIFETIME_MINUTES),
        }
    }

    fn expires_at(&self) -> DateTime<Utc> {
        match self {
            Self::DeviceCode { session, .. } => session.expires_at,
            Self::Pkce { expires_at, .. } => *expires_at,
        }
    }
}

pub(crate) struct PendingLoginStore {
    entries: Mutex<HashMap<String, PendingLogin>>,
    capacity: usize,
}

impl PendingLoginStore {
    pub(crate) fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            capacity: DEFAULT_PENDING_LOGIN_CAPACITY,
        }
    }

    pub(crate) fn insert(
        &self,
        session_id: &LoginSessionId,
        pending: PendingLogin,
    ) -> Result<(), AuthError> {
        self.insert_at(session_id, pending, Utc::now())
    }

    fn insert_at(
        &self,
        session_id: &LoginSessionId,
        pending: PendingLogin,
        now: DateTime<Utc>,
    ) -> Result<(), AuthError> {
        let mut entries = self.entries.lock().expect("pending login map");
        entries.retain(|_, item| item.expires_at() > now);
        if entries.len() >= self.capacity {
            return Err(AuthError::PendingLoginLimit);
        }
        entries.insert(session_id.as_str().to_string(), pending);
        Ok(())
    }

    pub(crate) fn get(&self, session_id: &LoginSessionId) -> Option<PendingLogin> {
        self.get_at(session_id, Utc::now())
    }

    fn get_at(&self, session_id: &LoginSessionId, now: DateTime<Utc>) -> Option<PendingLogin> {
        let mut entries = self.entries.lock().expect("pending login map");
        entries.retain(|_, item| item.expires_at() > now);
        entries.get(session_id.as_str()).cloned()
    }

    pub(crate) fn remove(&self, session_id: &LoginSessionId) -> Option<PendingLogin> {
        self.entries
            .lock()
            .expect("pending login map")
            .remove(session_id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkce(expires_at: DateTime<Utc>) -> PendingLogin {
        PendingLogin::Pkce {
            provider_alias: "example".into(),
            canonical: "example".into(),
            state: "secret-state".into(),
            session_data: serde_json::json!({"verifier": "secret"}),
            expires_at,
        }
    }

    #[test]
    fn hard_cap_rejects_new_session_without_exceeding_bound() {
        let store = PendingLoginStore {
            entries: Mutex::new(HashMap::new()),
            capacity: 2,
        };
        let now = Utc::now();
        store
            .insert_at(
                &LoginSessionId::new("one"),
                pkce(now + Duration::minutes(1)),
                now,
            )
            .unwrap();
        store
            .insert_at(
                &LoginSessionId::new("two"),
                pkce(now + Duration::minutes(1)),
                now,
            )
            .unwrap();

        assert!(matches!(
            store.insert_at(
                &LoginSessionId::new("three"),
                pkce(now + Duration::minutes(1)),
                now
            ),
            Err(AuthError::PendingLoginLimit)
        ));
        assert_eq!(store.entries.lock().unwrap().len(), 2);
    }

    #[test]
    fn expiry_sweep_frees_capacity_and_drops_stale_session() {
        let store = PendingLoginStore {
            entries: Mutex::new(HashMap::new()),
            capacity: 1,
        };
        let now = Utc::now();
        let stale_id = LoginSessionId::new("stale");
        store
            .insert_at(&stale_id, pkce(now + Duration::seconds(1)), now)
            .unwrap();
        let later = now + Duration::seconds(2);
        store
            .insert_at(
                &LoginSessionId::new("fresh"),
                pkce(later + Duration::seconds(1)),
                later,
            )
            .unwrap();

        assert!(store.get_at(&stale_id, later).is_none());
        assert_eq!(store.entries.lock().unwrap().len(), 1);
    }

    #[test]
    fn device_expiry_comes_from_device_session() {
        let now = Utc::now();
        let id = LoginSessionId::new("device");
        let store = PendingLoginStore::new();
        store
            .insert_at(
                &id,
                PendingLogin::DeviceCode {
                    provider_alias: "example".into(),
                    canonical: "example".into(),
                    session: DeviceCodeSession {
                        provider: "example".into(),
                        verification_url: "https://example.com".into(),
                        user_code: "CODE".into(),
                        device_code: "secret".into(),
                        interval_secs: 5,
                        expires_at: now + Duration::seconds(1),
                    },
                },
                now,
            )
            .unwrap();

        assert!(store.get_at(&id, now).is_some());
        assert!(store.get_at(&id, now + Duration::seconds(2)).is_none());
    }
}
