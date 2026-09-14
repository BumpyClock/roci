//! Host-safe login step/result types.
//!
//! These types intentionally omit raw tokens, device codes, PKCE verifiers,
//! session JSON, and API keys. Opaque session IDs refer to manager-owned
//! pending state.

use std::fmt;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Opaque pending-login session identifier.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LoginSessionId(String);

impl LoginSessionId {
    /// Wrap an existing opaque id (used by hosts when polling/completing).
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrow the opaque id string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for LoginSessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Ids are not secrets but avoid implying they embed material.
        f.debug_tuple("LoginSessionId").field(&self.0).finish()
    }
}

impl fmt::Display for LoginSessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for LoginSessionId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Host-visible outcome of starting a login flow.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostAuthStep {
    /// Open the URL and poll the opaque session; no code entry is required.
    BrowserPoll {
        authorization_url: String,
        interval_secs: u64,
        expires_at: DateTime<Utc>,
        session_id: LoginSessionId,
    },
    /// Credentials were imported; login is already complete.
    ImportedAndComplete {
        /// Canonical provider key.
        provider: String,
    },
    /// Device-code flow: show URL + user code, then poll with `session_id`.
    DeviceCode {
        verification_uri: String,
        user_code: String,
        interval_secs: u64,
        expires_at: DateTime<Utc>,
        session_id: LoginSessionId,
    },
    /// PKCE flow: open authorize URL, then complete with `session_id` + code.
    Pkce {
        authorization_url: String,
        session_id: LoginSessionId,
    },
}

impl fmt::Debug for HostAuthStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BrowserPoll {
                authorization_url,
                interval_secs,
                expires_at,
                session_id,
            } => f
                .debug_struct("BrowserPoll")
                .field("authorization_url", authorization_url)
                .field("interval_secs", interval_secs)
                .field("expires_at", expires_at)
                .field("session_id", session_id)
                .finish(),
            Self::ImportedAndComplete { provider } => f
                .debug_struct("ImportedAndComplete")
                .field("provider", provider)
                .finish(),
            Self::DeviceCode {
                verification_uri,
                user_code,
                interval_secs,
                expires_at,
                session_id,
            } => f
                .debug_struct("DeviceCode")
                .field("verification_uri", verification_uri)
                .field("user_code", user_code)
                .field("interval_secs", interval_secs)
                .field("expires_at", expires_at)
                .field("session_id", session_id)
                .finish(),
            Self::Pkce {
                authorization_url,
                session_id,
            } => f
                .debug_struct("Pkce")
                .field("authorization_url", authorization_url)
                .field("session_id", session_id)
                .finish(),
        }
    }
}

/// Host-visible device-code poll outcome (no token material).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostAuthPollResult {
    /// Still waiting for the user.
    Pending,
    /// Server asked the client to slow down.
    SlowDown { interval_secs: u64 },
    /// User authorized; Roci-owned credentials are stored.
    Authorized {
        /// Canonical provider key.
        provider: String,
    },
    /// User denied the request.
    Denied,
    /// Device code expired.
    Expired,
}

impl fmt::Debug for HostAuthPollResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "Pending"),
            Self::SlowDown { interval_secs } => f
                .debug_struct("SlowDown")
                .field("interval_secs", interval_secs)
                .finish(),
            Self::Authorized { provider } => f
                .debug_struct("Authorized")
                .field("provider", provider)
                .finish(),
            Self::Denied => write!(f, "Denied"),
            Self::Expired => write!(f, "Expired"),
        }
    }
}

/// Host-visible PKCE completion result (no token material).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAuthCompletion {
    /// Canonical provider key.
    pub provider: String,
}

impl fmt::Debug for HostAuthCompletion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostAuthCompletion")
            .field("provider", &self.provider)
            .finish()
    }
}

/// Convert a std duration to whole seconds for host payloads (ceil at least 1 when non-zero).
pub(crate) fn duration_secs(duration: Duration) -> u64 {
    let secs = duration.as_secs();
    if secs == 0 && !duration.is_zero() {
        1
    } else {
        secs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_auth_step_debug_preserves_pkce_fields() {
        let step = HostAuthStep::Pkce {
            authorization_url: "https://example.com/auth".into(),
            session_id: LoginSessionId::new("sess-1"),
        };
        let debug = format!("{step:?}");
        assert_eq!(
            debug,
            r#"Pkce { authorization_url: "https://example.com/auth", session_id: LoginSessionId("sess-1") }"#
        );
    }

    #[test]
    fn host_device_code_serializes_expected_wire_fields() {
        let step = HostAuthStep::DeviceCode {
            verification_uri: "https://example.com/device".into(),
            user_code: "ABCD-EFGH".into(),
            interval_secs: 5,
            expires_at: "2026-09-14T12:00:00Z".parse().unwrap(),
            session_id: LoginSessionId::new("sess-2"),
        };
        let json = serde_json::to_string(&step).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap(),
            serde_json::json!({
                "kind": "device_code",
                "verification_uri": "https://example.com/device",
                "user_code": "ABCD-EFGH",
                "interval_secs": 5,
                "expires_at": "2026-09-14T12:00:00Z",
                "session_id": "sess-2"
            })
        );
    }

    #[test]
    fn host_browser_poll_serializes_only_host_fields() {
        let step = HostAuthStep::BrowserPoll {
            authorization_url: "https://example.com/browser".into(),
            interval_secs: 1,
            expires_at: "2026-09-14T12:00:00Z".parse().unwrap(),
            session_id: LoginSessionId::new("opaque-session"),
        };
        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "browser_poll",
                "authorization_url": "https://example.com/browser",
                "interval_secs": 1,
                "expires_at": "2026-09-14T12:00:00Z",
                "session_id": "opaque-session"
            })
        );
        assert_eq!(serde_json::from_value::<HostAuthStep>(json).unwrap(), step);
    }
}
