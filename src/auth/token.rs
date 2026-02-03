use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// OAuth token payload stored in a token store.
///
/// # Example
/// ```no_run
/// use roci::auth::Token;
/// use chrono::{DateTime, Utc};
///
/// let token = Token {
///     access_token: "access".to_string(),
///     refresh_token: Some("refresh".to_string()),
///     id_token: None,
///     expires_at: None,
///     last_refresh: Some(DateTime::<Utc>::from(std::time::SystemTime::now())),
///     scopes: Some(vec!["openid".to_string()]),
///     account_id: None,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_refresh: Option<DateTime<Utc>>,
    pub scopes: Option<Vec<String>>,
    pub account_id: Option<String>,
}
