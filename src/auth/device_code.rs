use chrono::{DateTime, Utc};

use super::Token;

/// Device-code session details for OAuth providers.
///
/// # Example
/// ```no_run
/// use roci::auth::DeviceCodeSession;
/// use chrono::{DateTime, Utc};
///
/// let session = DeviceCodeSession {
///     provider: "openai-codex".to_string(),
///     verification_url: "https://auth.openai.com/codex/device".to_string(),
///     user_code: "ABCD-EFGH".to_string(),
///     device_code: "device-auth-id".to_string(),
///     interval_secs: 5,
///     expires_at: DateTime::<Utc>::from(std::time::SystemTime::now()),
/// };
/// ```
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
