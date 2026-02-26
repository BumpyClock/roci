//! CLI-specific error formatting for user-facing messages.

use roci::error::RociError;

/// Map a [`RociError`] to a user-facing help string with actionable guidance.
pub fn format_error_help(err: &RociError) -> String {
    match err {
        RociError::MissingCredential { provider } => {
            format!("Missing credentials for {provider}. Run: roci-agent auth login {provider}")
        }
        RociError::MissingConfiguration { key, provider } => {
            format!("Missing configuration '{key}' for {provider}. Run: roci-agent auth login {provider}")
        }
        RociError::Authentication(msg) => {
            format!("Authentication failed: {msg}. Run: roci-agent auth login <provider>")
        }
        RociError::Configuration(msg) => {
            format!("Configuration error: {msg}. Check your .env or run: roci-agent auth login <provider>")
        }
        other => format!("{other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_missing_credential_includes_provider() {
        let err = RociError::MissingCredential {
            provider: "copilot".to_string(),
        };
        let help = format_error_help(&err);
        assert!(help.contains("roci-agent auth login copilot"));
    }

    #[test]
    fn format_missing_configuration_includes_key_and_provider() {
        let err = RociError::MissingConfiguration {
            key: "base_url".to_string(),
            provider: "copilot".to_string(),
        };
        let help = format_error_help(&err);
        assert!(help.contains("base_url"));
        assert!(help.contains("roci-agent auth login copilot"));
    }

    #[test]
    fn format_authentication_error_includes_login_hint() {
        let err = RociError::Authentication("token expired".into());
        let help = format_error_help(&err);
        assert!(help.contains("roci-agent auth login"));
    }

    #[test]
    fn format_other_error_falls_through_to_display() {
        let err = RociError::ModelNotFound("gpt-99".into());
        let help = format_error_help(&err);
        assert!(help.contains("gpt-99"));
    }
}
