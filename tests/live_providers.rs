//! Live provider tests (ignored by default).

use roci::config::RociConfig;
use roci::generation;
use roci::models::{google, openai, LanguageModel};

fn print_result(label: &str, text: &str) {
    println!("{label} response:\n{text}");
}

#[tokio::test]
#[ignore]
async fn live_openai_gpt41_nano_generates_text() {
    let model = LanguageModel::OpenAi(openai::OpenAiModel::Gpt41Nano);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    let result = generation::generate_text(
        provider.as_ref(),
        vec![roci::types::ModelMessage::user("Say 'ok' and today's date")],
        roci::types::GenerationSettings::default(),
        &[],
    )
    .await
    .unwrap();
    print_result("openai:gpt-4.1-nano", &result.text);
    assert!(!result.text.trim().is_empty());
}

#[tokio::test]
#[ignore]
async fn live_openai_gpt5_nano_generates_text() {
    let model = LanguageModel::OpenAi(openai::OpenAiModel::Gpt5Nano);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    let result = generation::generate_text(
        provider.as_ref(),
        vec![roci::types::ModelMessage::user("Say 'ok' and today's date")],
        roci::types::GenerationSettings::default(),
        &[],
    )
    .await
    .unwrap();
    print_result("openai:gpt-5-nano", &result.text);
    assert!(!result.text.trim().is_empty());
}

#[tokio::test]
#[ignore]
async fn live_gemini_flash_preview_generates_text() {
    let model = LanguageModel::Google(google::GoogleModel::Gemini3FlashPreview);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    let result = generation::generate_text(
        provider.as_ref(),
        vec![roci::types::ModelMessage::user("Say 'ok' and today's date")],
        roci::types::GenerationSettings::default(),
        &[],
    )
    .await
    .unwrap();
    print_result("google:gemini-3-flash-preview", &result.text);
    assert!(!result.text.trim().is_empty());
}

#[tokio::test]
#[ignore]
async fn live_gemini_pro_preview_generates_text() {
    let model = LanguageModel::Google(google::GoogleModel::Gemini3ProPreview);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    let result = generation::generate_text(
        provider.as_ref(),
        vec![roci::types::ModelMessage::user("Say 'ok' and today's date")],
        roci::types::GenerationSettings::default(),
        &[],
    )
    .await
    .unwrap();
    print_result("google:gemini-3-pro-preview", &result.text);
    assert!(!result.text.trim().is_empty());
}

#[cfg(feature = "openai-compatible")]
#[tokio::test]
#[ignore]
async fn live_openai_compatible_generates_text() {
    let base_url = std::env::var("OPENAI_COMPAT_BASE_URL").unwrap();
    let model_id = std::env::var("OPENAI_COMPAT_MODEL").unwrap();
    let model = LanguageModel::OpenAiCompatible(
        roci::models::openai_compatible::OpenAiCompatibleModel::new(model_id, Some(base_url)),
    );
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    let result = generation::generate_text(
        provider.as_ref(),
        vec![roci::types::ModelMessage::user("Say 'ok' and today's date")],
        roci::types::GenerationSettings::default(),
        &[],
    )
    .await
    .unwrap();
    print_result("openai-compatible", &result.text);
    assert!(!result.text.trim().is_empty());
}
