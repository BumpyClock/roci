//! Tests for model system.

use roci::models::*;

#[test]
fn language_model_display() {
    let model = LanguageModel::OpenAi(openai::OpenAiModel::Gpt4o);
    assert_eq!(model.to_string(), "openai:gpt-4o");
}

#[test]
fn language_model_provider_name() {
    let model = LanguageModel::Anthropic(anthropic::AnthropicModel::ClaudeOpus45);
    assert_eq!(model.provider_name(), "anthropic");
    assert_eq!(model.model_id(), "claude-opus-4-5-20251101");
}

#[test]
fn openai_model_capabilities() {
    let model = openai::OpenAiModel::Gpt4o;
    let caps = model.capabilities();
    assert!(caps.supports_vision);
    assert!(caps.supports_tools);
    assert!(caps.supports_streaming);
    assert_eq!(caps.context_length, 128_000);
}

#[test]
fn anthropic_model_extended_thinking() {
    assert!(anthropic::AnthropicModel::ClaudeOpus45.supports_extended_thinking());
    assert!(!anthropic::AnthropicModel::Claude3Haiku.supports_extended_thinking());
}

#[test]
fn google_model_context_length() {
    let caps = google::GoogleModel::Gemini15Pro.capabilities();
    assert_eq!(caps.context_length, 2_000_000);
}

#[test]
fn openai_model_uses_responses_api() {
    assert!(openai::OpenAiModel::Gpt5.uses_responses_api());
    assert!(openai::OpenAiModel::O3.uses_responses_api());
    assert!(!openai::OpenAiModel::Gpt4o.uses_responses_api());
}

#[test]
fn openai_model_is_reasoning() {
    assert!(openai::OpenAiModel::O1.is_reasoning());
    assert!(openai::OpenAiModel::O3Mini.is_reasoning());
    assert!(!openai::OpenAiModel::Gpt4o.is_reasoning());
}

#[test]
fn custom_model() {
    let model = LanguageModel::Custom {
        provider: "mycloud".to_string(),
        model_id: "my-model-v1".to_string(),
    };
    assert_eq!(model.provider_name(), "mycloud");
    assert_eq!(model.model_id(), "my-model-v1");
}

#[test]
fn model_selector_parse() {
    let model = ModelSelector::parse("openai:gpt-4o").unwrap();
    assert_eq!(model.model_id(), "gpt-4o");
    assert_eq!(model.provider_name(), "openai");
}

#[test]
fn model_selector_from_str() {
    let model: LanguageModel = "anthropic:claude-opus-4-5-20251101".parse().unwrap();
    assert_eq!(model.provider_name(), "anthropic");
}

#[test]
fn model_selector_unknown_provider() {
    let model = ModelSelector::parse("custom:my-model").unwrap();
    assert_eq!(model.provider_name(), "custom");
    assert_eq!(model.model_id(), "my-model");
}

#[test]
fn model_selector_invalid() {
    assert!(ModelSelector::parse("no-colon").is_err());
}

#[test]
fn model_selector_parses_gpt5_nano() {
    let model = ModelSelector::parse("openai:gpt-5-nano").unwrap();
    assert_eq!(model.provider_name(), "openai");
    assert_eq!(model.model_id(), "gpt-5-nano");
}

#[test]
fn model_selector_parses_gemini_preview_models() {
    let flash = ModelSelector::parse("google:gemini-3-flash-preview").unwrap();
    assert_eq!(flash.provider_name(), "google");
    assert_eq!(flash.model_id(), "gemini-3-flash-preview");

    let pro = ModelSelector::parse("google:gemini-3-pro-preview").unwrap();
    assert_eq!(pro.provider_name(), "google");
    assert_eq!(pro.model_id(), "gemini-3-pro-preview");
}
