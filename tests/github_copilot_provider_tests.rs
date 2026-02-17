#[cfg(feature = "openai-compatible")]
use roci::config::RociConfig;
#[cfg(feature = "openai-compatible")]
use roci::models::LanguageModel;
#[cfg(feature = "openai-compatible")]
use roci::provider::create_provider;

#[cfg(feature = "openai-compatible")]
#[test]
fn github_copilot_provider_accepts_dedicated_config_keys() {
    let model: LanguageModel = "github-copilot:gpt-4.1".parse().expect("model parse");
    let config = RociConfig::new();
    config.set_api_key("github-copilot", "token".to_string());
    config.set_base_url("github-copilot", "https://example.test/v1".to_string());

    let provider = create_provider(&model, &config).expect("provider");
    assert_eq!(provider.provider_name(), "github-copilot");
}

#[cfg(feature = "openai-compatible")]
#[test]
fn github_copilot_provider_does_not_fall_back_to_openai_compat_keys() {
    let model: LanguageModel = "github-copilot:gpt-4.1".parse().expect("model parse");
    let config = RociConfig::new();
    config.set_api_key("openai-compatible", "token".to_string());
    config.set_base_url("openai-compatible", "https://example.test/v1".to_string());

    let err = match create_provider(&model, &config) {
        Ok(_) => panic!("expected missing key error"),
        Err(err) => err,
    };
    let text = err.to_string();
    assert!(
        text.contains("GITHUB_COPILOT_API_KEY"),
        "unexpected error: {text}"
    );
}

#[cfg(feature = "openai-compatible")]
#[test]
fn github_copilot_provider_requires_credentials() {
    let model: LanguageModel = "github-copilot:gpt-4.1".parse().expect("model parse");
    let config = RociConfig::new();

    let err = match create_provider(&model, &config) {
        Ok(_) => panic!("expected missing key error"),
        Err(err) => err,
    };
    let text = err.to_string();
    assert!(
        text.contains("GITHUB_COPILOT_API_KEY"),
        "unexpected error: {text}"
    );
}
