//! OAuth token payload.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// OAuth token payload stored in a token store.
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
