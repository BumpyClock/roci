//! Tests for configuration system.

use roci::config::RociConfig;

#[test]
fn config_set_get_api_key() {
    let config = RociConfig::new();
    config.set_api_key("openai", "sk-test-123".to_string());
    assert_eq!(
        config.get_api_key("openai"),
        Some("sk-test-123".to_string())
    );
    assert_eq!(config.get_api_key("anthropic"), None);
}

#[test]
fn config_set_get_base_url() {
    let config = RociConfig::new();
    config.set_base_url("openai", "http://localhost:8080".to_string());
    assert_eq!(
        config.get_base_url("openai"),
        Some("http://localhost:8080".to_string())
    );
}

#[test]
fn config_has_credentials() {
    let config = RociConfig::new();
    assert!(!config.has_credentials("openai"));
    config.set_api_key("openai", "sk-test".to_string());
    assert!(config.has_credentials("openai"));
}

#[test]
fn config_from_env_loads_keys() {
    // Set env vars for test
    std::env::set_var("OPENAI_API_KEY", "test-openai-key");
    std::env::set_var("ANTHROPIC_API_KEY", "test-anthropic-key");

    let config = RociConfig::from_env();
    assert_eq!(
        config.get_api_key("openai"),
        Some("test-openai-key".to_string())
    );
    assert_eq!(
        config.get_api_key("anthropic"),
        Some("test-anthropic-key".to_string())
    );

    // Clean up
    std::env::remove_var("OPENAI_API_KEY");
    std::env::remove_var("ANTHROPIC_API_KEY");
}
