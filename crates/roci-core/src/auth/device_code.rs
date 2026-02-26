//! Device-code session types.

use chrono::{DateTime, Utc};

use super::Token;

/// Device-code session details for OAuth providers.
#[derive(Debug, Clone)]
pub struct DeviceCodeSession {
    pub provider: String,
    pub verification_url: String,
    pub user_code: String,
    pub device_code: String,
    pub interval_secs: u64,
    pub expires_at: DateTime<Utc>,
}

/// Polling outcome for a device-code session.
#[derive(Debug, Clone)]
pub enum DeviceCodePoll {
    Pending { interval_secs: u64 },
    SlowDown { interval_secs: u64 },
    Authorized { token: Token },
    AccessDenied,
    Expired,
}
