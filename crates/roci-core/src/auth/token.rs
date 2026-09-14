//! OAuth token payload.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Provider-owned transport metadata persisted with the OAuth credential.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderTokenMetadata {
    Gemini { project_id: String },
}

impl fmt::Debug for ProviderTokenMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gemini { .. } => f.write_str("Gemini { project_id: <redacted> }"),
        }
    }
}

/// OAuth token payload stored in a token store.
///
/// Serialization retains full fields for credential storage. [`Debug`] always
/// redacts secret-bearing fields so logs never print raw tokens.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderTokenMetadata>,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_refresh: Option<DateTime<Utc>>,
    pub scopes: Option<Vec<String>>,
    /// May hold non-account transport metadata (e.g. Copilot base URL).
    pub account_id: Option<String>,
}

impl Token {
    /// Whether the access token is unexpired at the current time.
    ///
    /// Tokens without an expiry remain valid because some OAuth backends manage
    /// refresh or validity outside the serialized access-token payload.
    pub fn is_valid(&self) -> bool {
        !self.access_token.trim().is_empty()
            && self
                .expires_at
                .map(|expires_at| expires_at > Utc::now())
                .unwrap_or(true)
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Token")
            .field("provider_metadata", &self.provider_metadata)
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "<redacted>"))
            .field("expires_at", &self.expires_at)
            .field("last_refresh", &self.last_refresh)
            .field("scopes", &self.scopes)
            .field(
                "account_id",
                &self.account_id.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_secret_fields() {
        let token = Token {
            provider_metadata: None,
            access_token: "access-secret".into(),
            refresh_token: Some("refresh-secret".into()),
            id_token: Some("id-secret".into()),
            expires_at: None,
            last_refresh: None,
            scopes: Some(vec!["read".into()]),
            account_id: Some("https://api.example/account".into()),
        };
        let debug = format!("{token:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("access-secret"));
        assert!(!debug.contains("refresh-secret"));
        assert!(!debug.contains("id-secret"));
        assert!(!debug.contains("api.example"));
        assert!(debug.contains("read"));
    }

    #[test]
    fn provider_metadata_is_optional_durable_and_debug_redacted() {
        let mut token: Token =
            serde_json::from_value(serde_json::json!({"access_token":"a"})).unwrap();
        assert!(token.provider_metadata.is_none());
        assert!(serde_json::to_value(&token)
            .unwrap()
            .get("provider_metadata")
            .is_none());
        token.provider_metadata = Some(ProviderTokenMetadata::Gemini {
            project_id: "private-project".into(),
        });
        let encoded = toml::to_string(&token).unwrap();
        assert_eq!(toml::from_str::<Token>(&encoded).unwrap(), token);
        assert!(!format!("{token:?}").contains("private-project"));
        assert!(!format!("{:?}", token.provider_metadata).contains("private-project"));
    }
}
