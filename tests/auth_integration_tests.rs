//! Integration tests for auth subsystem: token store round-trips,
//! config fallback resolution, and CLI argument parsing.

use std::sync::Arc;

use chrono::{Duration, Utc};
use tempfile::TempDir;

use roci::auth::store::{FileTokenStore, TokenStore, TokenStoreConfig};
use roci::auth::token::Token;
use roci::config::RociConfig;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn temp_store() -> (TempDir, FileTokenStore) {
    let dir = TempDir::new().expect("tempdir");
    let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
    (dir, store)
}

fn make_token(
    access: &str,
    refresh: Option<&str>,
    expires_at: Option<chrono::DateTime<Utc>>,
) -> Token {
    Token {
        access_token: access.to_string(),
        refresh_token: refresh.map(String::from),
        id_token: None,
        expires_at,
        last_refresh: Some(Utc::now()),
        scopes: None,
        account_id: None,
    }
}

// ---------------------------------------------------------------------------
// 1. Token store round-trip
// ---------------------------------------------------------------------------

#[test]
fn token_store_round_trip_preserves_all_fields() {
    let (_dir, store) = temp_store();

    let future = Utc::now() + Duration::hours(2);
    let original = Token {
        access_token: "acc-123".to_string(),
        refresh_token: Some("ref-456".to_string()),
        id_token: Some("id-789".to_string()),
        expires_at: Some(future),
        last_refresh: Some(Utc::now()),
        scopes: Some(vec!["read".to_string(), "write".to_string()]),
        account_id: Some("acct-000".to_string()),
    };

    store
        .save("test-provider", "default", &original)
        .expect("save should succeed");

    let loaded = store
        .load("test-provider", "default")
        .expect("load should succeed")
        .expect("token should exist");

    assert_eq!(loaded.access_token, original.access_token);
    assert_eq!(loaded.refresh_token, original.refresh_token);
    assert_eq!(loaded.id_token, original.id_token);
    assert_eq!(
        loaded.expires_at.map(|t| t.timestamp()),
        original.expires_at.map(|t| t.timestamp()),
    );
    assert!(loaded.last_refresh.is_some());
    assert_eq!(loaded.scopes, original.scopes);
    assert_eq!(loaded.account_id, original.account_id);
}

#[test]
fn token_store_load_missing_returns_none() {
    let (_dir, store) = temp_store();
    let result = store
        .load("nonexistent", "default")
        .expect("load should succeed");
    assert!(result.is_none());
}

#[test]
fn token_store_clear_removes_token() {
    let (_dir, store) = temp_store();
    let token = make_token("tok", None, None);

    store.save("prov", "default", &token).unwrap();
    assert!(store.load("prov", "default").unwrap().is_some());

    store.clear("prov", "default").unwrap();
    assert!(store.load("prov", "default").unwrap().is_none());
}

#[test]
fn token_store_clear_missing_is_noop() {
    let (_dir, store) = temp_store();
    // Should not error when clearing a token that was never saved.
    store.clear("ghost", "default").unwrap();
}

// ---------------------------------------------------------------------------
// 2. Config fallback resolution
// ---------------------------------------------------------------------------

#[test]
fn config_get_api_key_falls_back_to_token_store() {
    let (dir, store) = temp_store();

    let token = make_token("claude-oauth-tok", None, None);
    store.save("claude-code", "default", &token).unwrap();

    let store_for_config = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
    let config = RociConfig::new().with_token_store(Some(Arc::new(store_for_config)));

    assert_eq!(
        config.get_api_key("anthropic"),
        Some("claude-oauth-tok".to_string()),
    );
}

#[test]
fn config_explicit_key_takes_precedence_over_store() {
    let (dir, store) = temp_store();

    let token = make_token("stored-key", None, None);
    store.save("openai-codex", "default", &token).unwrap();

    let store_for_config = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
    let config = RociConfig::new().with_token_store(Some(Arc::new(store_for_config)));
    config.set_api_key("openai", "explicit-key".to_string());

    assert_eq!(
        config.get_api_key("openai"),
        Some("explicit-key".to_string()),
    );
}

#[test]
fn config_expired_token_not_returned() {
    let (dir, store) = temp_store();

    let expired = Utc::now() - Duration::hours(1);
    let token = make_token("old-tok", None, Some(expired));
    store.save("openai-codex", "default", &token).unwrap();

    let store_for_config = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
    let config = RociConfig::new().with_token_store(Some(Arc::new(store_for_config)));

    assert_eq!(config.get_api_key("openai"), None);
}

#[test]
fn config_non_expired_token_is_returned() {
    let (dir, store) = temp_store();

    let future = Utc::now() + Duration::hours(1);
    let token = make_token("fresh-tok", None, Some(future));
    store.save("openai-codex", "default", &token).unwrap();

    let store_for_config = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
    let config = RociConfig::new().with_token_store(Some(Arc::new(store_for_config)));

    assert_eq!(config.get_api_key("codex"), Some("fresh-tok".to_string()),);
}

// ---------------------------------------------------------------------------
// 3. CLI argument parsing
// ---------------------------------------------------------------------------

#[cfg(feature = "cli")]
mod cli_parse {
    use clap::Parser;
    use roci::cli::{AuthCommands, Cli, Commands};

    #[test]
    fn parse_auth_login_claude() {
        let cli =
            Cli::try_parse_from(["roci", "auth", "login", "claude"]).expect("parse should succeed");
        match cli.command {
            Commands::Auth(auth) => match auth.command {
                AuthCommands::Login(args) => assert_eq!(args.provider, "claude"),
                other => panic!("expected Login, got {other:?}"),
            },
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn parse_auth_status() {
        let cli = Cli::try_parse_from(["roci", "auth", "status"]).expect("parse should succeed");
        match cli.command {
            Commands::Auth(auth) => {
                assert!(matches!(auth.command, AuthCommands::Status));
            }
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn parse_auth_logout_claude() {
        let cli = Cli::try_parse_from(["roci", "auth", "logout", "claude"])
            .expect("parse should succeed");
        match cli.command {
            Commands::Auth(auth) => match auth.command {
                AuthCommands::Logout(args) => assert_eq!(args.provider, "claude"),
                other => panic!("expected Logout, got {other:?}"),
            },
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn parse_auth_login_copilot() {
        let cli = Cli::try_parse_from(["roci", "auth", "login", "copilot"])
            .expect("parse should succeed");
        match cli.command {
            Commands::Auth(auth) => match auth.command {
                AuthCommands::Login(args) => assert_eq!(args.provider, "copilot"),
                other => panic!("expected Login, got {other:?}"),
            },
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn parse_auth_login_chatgpt() {
        let cli = Cli::try_parse_from(["roci", "auth", "login", "chatgpt"])
            .expect("parse should succeed");
        match cli.command {
            Commands::Auth(auth) => match auth.command {
                AuthCommands::Login(args) => assert_eq!(args.provider, "chatgpt"),
                other => panic!("expected Login, got {other:?}"),
            },
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn parse_auth_login_missing_provider_is_error() {
        assert!(Cli::try_parse_from(["roci", "auth", "login"]).is_err());
    }
}
