//! Bounded, expiring storage for secret pending-login material.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};

use super::device_code::DeviceCodeSession;
use super::error::AuthError;
use super::host::LoginSessionId;

const MAX_PENDING_LOGINS: usize = 32;
const PKCE_SESSION_LIFETIME_MINUTES: i64 = 10;

/// Internal pending login material that must never cross the manager boundary.
#[derive(Clone)]
pub(crate) enum PendingLogin {
    BrowserPoll {
        provider_alias: String,
        canonical: String,
        session_data: serde_json::Value,
        expires_at: DateTime<Utc>,
    },
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
            Self::BrowserPoll { expires_at, .. } => *expires_at,
        }
    }
}

struct PendingLoginEntry {
    pending: PendingLogin,
    claimed: bool,
}

pub(crate) struct PendingLoginStore {
    entries: Mutex<HashMap<String, PendingLoginEntry>>,
}

/// Exclusive, cancellation-safe claim on one pending login.
pub(crate) struct PendingLoginClaim<'a> {
    store: &'a PendingLoginStore,
    session_id: String,
    pending: PendingLogin,
    consumed: bool,
}

impl PendingLoginClaim<'_> {
    pub(crate) fn pending(&self) -> &PendingLogin {
        &self.pending
    }

    pub(crate) fn consume(mut self) {
        self.store.remove(&self.session_id);
        self.consumed = true;
    }
}

impl Drop for PendingLoginClaim<'_> {
    fn drop(&mut self) {
        if !self.consumed {
            self.store.release(&self.session_id, Utc::now());
        }
    }
}

impl PendingLoginStore {
    pub(crate) fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
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
        entries.retain(|_, entry| entry.pending.expires_at() > now);
        if entries.len() >= MAX_PENDING_LOGINS {
            return Err(AuthError::PendingLoginLimit);
        }
        entries.insert(
            session_id.as_str().to_string(),
            PendingLoginEntry {
                pending,
                claimed: false,
            },
        );
        Ok(())
    }

    pub(crate) fn claim(
        &self,
        session_id: &LoginSessionId,
    ) -> Result<PendingLoginClaim<'_>, AuthError> {
        let now = Utc::now();
        let mut entries = self.entries.lock().expect("pending login map");
        entries.retain(|_, entry| entry.pending.expires_at() > now);
        let entry = entries
            .get_mut(session_id.as_str())
            .ok_or_else(|| AuthError::InvalidResponse("unknown or expired login session".into()))?;
        if entry.claimed {
            return Err(AuthError::InvalidResponse(
                "login session is already in progress".into(),
            ));
        }
        entry.claimed = true;
        let pending = entry.pending.clone();
        drop(entries);
        Ok(PendingLoginClaim {
            store: self,
            session_id: session_id.as_str().to_string(),
            pending,
            consumed: false,
        })
    }

    fn release(&self, session_id: &str, now: DateTime<Utc>) {
        let mut entries = self.entries.lock().expect("pending login map");
        let expired = entries
            .get(session_id)
            .is_some_and(|entry| entry.pending.expires_at() <= now);
        if expired {
            entries.remove(session_id);
        } else if let Some(entry) = entries.get_mut(session_id) {
            entry.claimed = false;
        }
    }

    fn remove(&self, session_id: &str) -> Option<PendingLogin> {
        self.entries
            .lock()
            .expect("pending login map")
            .remove(session_id)
            .map(|entry| entry.pending)
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

    fn fill_sessions(store: &PendingLoginStore, count: usize, now: DateTime<Utc>) {
        for index in 0..count {
            store
                .insert_at(
                    &LoginSessionId::new(format!("existing-{index}")),
                    pkce(now + Duration::minutes(10)),
                    now,
                )
                .expect("session should fit within the fixed cap");
        }
    }

    #[test]
    fn hard_cap_rejects_new_session_without_exceeding_bound() {
        let store = PendingLoginStore::new();
        let now = Utc::now();
        fill_sessions(&store, 32, now);

        assert!(matches!(
            store.insert_at(
                &LoginSessionId::new("overflow"),
                pkce(now + Duration::minutes(10)),
                now
            ),
            Err(AuthError::PendingLoginLimit)
        ));
        assert_eq!(store.entries.lock().unwrap().len(), 32);
    }

    #[test]
    fn expiry_sweep_frees_capacity_and_drops_stale_session() {
        let store = PendingLoginStore::new();
        let now = Utc::now();
        fill_sessions(&store, 31, now);
        let stale_id = LoginSessionId::new("stale");
        store
            .insert_at(&stale_id, pkce(now + Duration::seconds(1)), now)
            .unwrap();
        let later = now + Duration::seconds(2);
        let fresh_id = LoginSessionId::new("fresh");
        store
            .insert_at(&fresh_id, pkce(later + Duration::minutes(10)), later)
            .unwrap();

        assert!(store.claim(&stale_id).is_err());
        assert!(store.claim(&fresh_id).is_ok());
        assert_eq!(store.entries.lock().unwrap().len(), 32);
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

        let claim = store.claim(&id).unwrap();
        assert!(matches!(claim.pending(), PendingLogin::DeviceCode { .. }));
        drop(claim);
        store.release(id.as_str(), now + Duration::seconds(2));
        assert!(store.claim(&id).is_err());
    }

    #[test]
    fn claim_is_exclusive_and_drop_restores_session() {
        let store = PendingLoginStore::new();
        let id = LoginSessionId::new("exclusive");
        store
            .insert(&id, pkce(Utc::now() + Duration::minutes(1)))
            .unwrap();

        let first = store.claim(&id).unwrap();
        assert!(matches!(
            store.claim(&id),
            Err(AuthError::InvalidResponse(ref message))
                if message == "login session is already in progress"
        ));
        drop(first);

        assert!(store.claim(&id).is_ok());
    }

    #[test]
    fn claimed_session_counts_toward_capacity_and_expiry_is_not_restored() {
        let store = PendingLoginStore::new();
        let now = Utc::now();
        fill_sessions(&store, 31, now);
        let claimed_id = LoginSessionId::new("claimed");
        store
            .insert_at(&claimed_id, pkce(now + Duration::minutes(1)), now)
            .unwrap();
        let claim = store.claim(&claimed_id).unwrap();

        assert!(matches!(
            store.insert_at(
                &LoginSessionId::new("other"),
                pkce(now + Duration::minutes(10)),
                now,
            ),
            Err(AuthError::PendingLoginLimit)
        ));
        store.release(claimed_id.as_str(), now + Duration::minutes(2));
        drop(claim);

        assert!(store.claim(&claimed_id).is_err());
        assert!(store
            .insert_at(
                &LoginSessionId::new("other"),
                pkce(now + Duration::minutes(10)),
                now + Duration::minutes(2),
            )
            .is_ok());
    }
}
