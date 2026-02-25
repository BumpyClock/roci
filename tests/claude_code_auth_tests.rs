mod auth_support;

use std::fs;
use std::sync::Arc;

use roci::auth::providers::claude_code::ClaudeCodeAuth;
use roci::auth::AuthError;
use tempfile::TempDir;

use auth_support::InMemoryTokenStore;

fn write_claude_credentials(temp: &TempDir, contents: &str) {
    let path = temp.path().join(".claude");
    fs::create_dir_all(&path).expect("create .claude dir");
    fs::write(path.join(".credentials.json"), contents).expect("write credentials");
}

#[tokio::test]
async fn claude_import_missing_file_returns_none() {
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store.clone());
    let temp = TempDir::new().expect("tempdir");

    let imported = auth
        .import_cli_credentials(Some(temp.path().to_path_buf()))
        .expect("import should not fail");

    assert!(imported.is_none());
    assert!(!auth.logged_in().await.expect("logged_in"));
    assert!(matches!(
        auth.get_token().await,
        Err(AuthError::NotLoggedIn)
    ));
}

#[tokio::test]
async fn claude_import_without_oauth_payload_returns_none() {
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store.clone());
    let temp = TempDir::new().expect("tempdir");
    write_claude_credentials(&temp, r#"{"claudeAiOauth":null}"#);

    let imported = auth
        .import_cli_credentials(Some(temp.path().to_path_buf()))
        .expect("import should not fail");

    assert!(imported.is_none());
    assert!(!auth.logged_in().await.expect("logged_in"));
    assert!(store.get("claude-code", "default").is_none());
}

#[tokio::test]
async fn claude_import_success_persists_token_and_get_token_works() {
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store.clone());
    let temp = TempDir::new().expect("tempdir");

    write_claude_credentials(
        &temp,
        r#"{
          "claudeAiOauth": {
            "accessToken": "claude-access",
            "refreshToken": "claude-refresh",
            "expiresAt": 1728000000000
          }
        }"#,
    );

    let imported = auth
        .import_cli_credentials(Some(temp.path().to_path_buf()))
        .expect("import should succeed")
        .expect("token should be imported");

    assert_eq!(imported.access_token, "claude-access");
    assert_eq!(imported.refresh_token.as_deref(), Some("claude-refresh"));
    assert_eq!(
        imported.expires_at.expect("expires_at").timestamp(),
        1_728_000_000
    );
    assert!(imported.last_refresh.is_some());

    assert!(auth.logged_in().await.expect("logged_in"));
    let loaded = auth.get_token().await.expect("token should exist");
    assert_eq!(loaded.access_token, "claude-access");
    assert_eq!(loaded.refresh_token.as_deref(), Some("claude-refresh"));
}

#[tokio::test]
async fn claude_import_honors_profile_isolation() {
    let store = Arc::new(InMemoryTokenStore::new());
    let default_auth = ClaudeCodeAuth::new(store.clone());
    let work_auth = ClaudeCodeAuth::new(store.clone()).with_profile("work");
    let temp = TempDir::new().expect("tempdir");

    write_claude_credentials(
        &temp,
        r#"{
          "claudeAiOauth": {
            "accessToken": "work-access",
            "refreshToken": "work-refresh",
            "expiresAt": 1728000000000
          }
        }"#,
    );

    let imported = work_auth
        .import_cli_credentials(Some(temp.path().to_path_buf()))
        .expect("import should succeed");

    assert!(imported.is_some());
    assert!(!default_auth.logged_in().await.expect("default logged_in"));
    assert!(work_auth.logged_in().await.expect("work logged_in"));
    assert_eq!(
        store
            .get("claude-code", "work")
            .expect("work token should exist")
            .access_token,
        "work-access"
    );
}

#[tokio::test]
async fn claude_import_invalid_json_returns_serialization_error() {
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store);
    let temp = TempDir::new().expect("tempdir");
    write_claude_credentials(&temp, "{not-json");

    let result = auth.import_cli_credentials(Some(temp.path().to_path_buf()));

    assert!(matches!(result, Err(AuthError::Serialization(_))));
}
