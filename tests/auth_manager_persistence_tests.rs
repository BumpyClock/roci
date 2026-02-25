use std::fs;
use std::path::PathBuf;

use roci::config::{AuthManager, AuthValue};
use roci::error::RociError;
use tempfile::TempDir;

fn credentials_path(temp_dir: &TempDir) -> PathBuf {
    temp_dir.path().join("credentials.json")
}

#[test]
fn auth_manager_round_trip_persists_all_auth_value_variants() {
    let temp_dir = TempDir::new().unwrap();
    let path = credentials_path(&temp_dir);

    let mut manager = AuthManager::new();
    manager.set("openai", AuthValue::ApiKey("sk-openai".to_string()));
    manager.set(
        "anthropic",
        AuthValue::BearerToken("bearer-token".to_string()),
    );
    manager.set("google", AuthValue::EnvVar("GOOGLE_API_KEY".to_string()));
    manager.save_to_path(&path).unwrap();

    let raw = fs::read_to_string(&path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["credentials"]["openai"]["type"], "api_key");
    assert_eq!(json["credentials"]["openai"]["value"], "sk-openai");
    assert_eq!(json["credentials"]["anthropic"]["type"], "bearer_token");
    assert_eq!(json["credentials"]["anthropic"]["value"], "bearer-token");
    assert_eq!(json["credentials"]["google"]["type"], "env_var");
    assert_eq!(json["credentials"]["google"]["value"], "GOOGLE_API_KEY");

    let loaded = AuthManager::load_from_path(&path).unwrap();
    assert_eq!(
        loaded.get("openai"),
        Some(&AuthValue::ApiKey("sk-openai".to_string()))
    );
    assert_eq!(
        loaded.get("anthropic"),
        Some(&AuthValue::BearerToken("bearer-token".to_string()))
    );
    assert_eq!(
        loaded.get("google"),
        Some(&AuthValue::EnvVar("GOOGLE_API_KEY".to_string()))
    );
}

#[test]
fn auth_manager_save_to_path_overwrites_existing_content() {
    let temp_dir = TempDir::new().unwrap();
    let path = credentials_path(&temp_dir);

    let mut first = AuthManager::new();
    first.set("openai", AuthValue::ApiKey("first".to_string()));
    first.set("legacy", AuthValue::ApiKey("remove-me".to_string()));
    first.save_to_path(&path).unwrap();

    let mut second = AuthManager::new();
    second.set("openai", AuthValue::ApiKey("second".to_string()));
    second.save_to_path(&path).unwrap();

    let loaded = AuthManager::load_from_path(&path).unwrap();
    assert_eq!(
        loaded.get("openai"),
        Some(&AuthValue::ApiKey("second".to_string()))
    );
    assert_eq!(loaded.get("legacy"), None);
}

#[test]
fn auth_manager_load_from_path_missing_file_returns_empty_manager() {
    let temp_dir = TempDir::new().unwrap();
    let path = credentials_path(&temp_dir);

    let loaded = AuthManager::load_from_path(&path).unwrap();
    assert_eq!(loaded.get("openai"), None);
    assert_eq!(loaded.get("anthropic"), None);
}

#[test]
fn auth_manager_load_from_path_corrupt_json_returns_error() {
    let temp_dir = TempDir::new().unwrap();
    let path = credentials_path(&temp_dir);
    fs::write(&path, "{not-json").unwrap();

    let err = AuthManager::load_from_path(&path).unwrap_err();
    assert!(matches!(err, RociError::Serialization(_)));
}

#[test]
fn auth_manager_save_to_path_does_not_leave_temp_files() {
    let temp_dir = TempDir::new().unwrap();
    let path = credentials_path(&temp_dir);

    let mut manager = AuthManager::new();
    manager.set("openai", AuthValue::ApiKey("sk-test".to_string()));
    manager.save_to_path(&path).unwrap();

    assert!(path.exists());
    let has_tmp = fs::read_dir(temp_dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .any(|name| name.contains(".tmp-"));
    assert!(!has_tmp);
}

#[cfg(unix)]
#[test]
fn auth_manager_save_to_path_sets_unix_permissions_to_0600() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = TempDir::new().unwrap();
    let path = credentials_path(&temp_dir);

    let mut manager = AuthManager::new();
    manager.set("openai", AuthValue::ApiKey("sk-test".to_string()));
    manager.save_to_path(&path).unwrap();

    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}
