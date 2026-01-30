//! Live provider tests (ignored by default).

use roci::config::RociConfig;
use roci::generation;
use roci::models::{google, openai, LanguageModel};
use roci::tools::{AgentTool, AgentToolParameters};
use roci::types::{GenerationSettings, ModelMessage, TextVerbosity};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn print_result(label: &str, text: &str) {
    println!("{label} response:\n{text}");
    let _ = std::io::stdout().flush();
}

async fn run_tool_flow_test(
    provider: &dyn roci::provider::ModelProvider,
    label: &str,
) -> roci::error::Result<roci::types::GenerateTextResult> {
    let called = Arc::new(AtomicUsize::new(0));
    let called_ref = Arc::clone(&called);
    let tool: Arc<dyn roci::tools::Tool> = Arc::new(AgentTool::new(
        "get_date",
        "Return ok and a date label",
        AgentToolParameters::empty(),
        move |_args, _ctx| {
            let called_ref = Arc::clone(&called_ref);
            async move {
                called_ref.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({"ok": true, "date": "today"}))
            }
        },
    ));
    let result = generation::generate_text(
        provider,
        vec![ModelMessage::user(
            "Call the tool `get_date` exactly once and reply with the tool output only.",
        )],
        GenerationSettings::default(),
        &[tool],
    )
    .await?;
    print_result(label, &result.text);
    assert!(called.load(Ordering::SeqCst) > 0);
    assert!(result.steps.iter().any(|step| !step.tool_calls.is_empty()));
    assert!(result
        .steps
        .iter()
        .any(|step| !step.tool_results.is_empty()));
    Ok(result)
}

#[tokio::test]
#[ignore]
async fn live_openai_gpt41_nano_generates_text() {
    let model = LanguageModel::OpenAi(openai::OpenAiModel::Gpt41Nano);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    let result = generation::generate_text(
        provider.as_ref(),
        vec![ModelMessage::user("Say 'ok' and today's date")],
        GenerationSettings::default(),
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
        vec![ModelMessage::user("Say 'ok' and today's date")],
        GenerationSettings {
            text_verbosity: Some(TextVerbosity::Low),
            ..Default::default()
        },
        &[],
    )
    .await
    .unwrap();
    print_result("openai:gpt-5-nano", &result.text);
    assert!(!result.text.trim().is_empty());
}

#[tokio::test]
#[ignore]
async fn live_openai_gpt41_nano_executes_tool_call() {
    let model = LanguageModel::OpenAi(openai::OpenAiModel::Gpt41Nano);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_tool_flow_test(provider.as_ref(), "openai:gpt-4.1-nano tool")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_openai_gpt5_nano_executes_tool_call() {
    let model = LanguageModel::OpenAi(openai::OpenAiModel::Gpt5Nano);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_tool_flow_test(provider.as_ref(), "openai:gpt-5-nano tool")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_gemini_flash_preview_generates_text() {
    let model = LanguageModel::Google(google::GoogleModel::Gemini3FlashPreview);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    let result = generation::generate_text(
        provider.as_ref(),
        vec![ModelMessage::user("Say 'ok' and today's date")],
        GenerationSettings::default(),
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
        vec![ModelMessage::user("Say 'ok' and today's date")],
        GenerationSettings::default(),
        &[],
    )
    .await
    .unwrap();
    print_result("google:gemini-3-pro-preview", &result.text);
    assert!(!result.text.trim().is_empty());
}

#[tokio::test]
#[ignore]
async fn live_gemini_flash_preview_executes_tool_call() {
    let model = LanguageModel::Google(google::GoogleModel::Gemini3FlashPreview);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_tool_flow_test(provider.as_ref(), "google:gemini-3-flash-preview tool")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_gemini_pro_preview_executes_tool_call() {
    let model = LanguageModel::Google(google::GoogleModel::Gemini3ProPreview);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_tool_flow_test(provider.as_ref(), "google:gemini-3-pro-preview tool")
        .await
        .unwrap();
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
        vec![ModelMessage::user("Say 'ok' and today's date")],
        GenerationSettings::default(),
        &[],
    )
    .await
    .unwrap();
    print_result("openai-compatible", &result.text);
    assert!(!result.text.trim().is_empty());
}

#[cfg(feature = "openai-compatible")]
#[tokio::test]
#[ignore]
async fn live_openai_compatible_executes_tool_call() {
    let base_url = std::env::var("OPENAI_COMPAT_BASE_URL").unwrap();
    let model_id = std::env::var("OPENAI_COMPAT_MODEL").unwrap();
    let model = LanguageModel::OpenAiCompatible(
        roci::models::openai_compatible::OpenAiCompatibleModel::new(model_id, Some(base_url)),
    );
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_tool_flow_test(provider.as_ref(), "openai-compatible tool")
        .await
        .unwrap();
}
