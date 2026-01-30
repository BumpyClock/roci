//! Live provider tests (ignored by default).

use roci::config::RociConfig;
use roci::generation;
use roci::generation::stream::collect_stream;
use roci::models::{google, openai, LanguageModel};
use roci::tools::{AgentTool, AgentToolParameters};
use roci::types::{GenerationSettings, ModelMessage, TextVerbosity};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;

const RED_PIXEL_PNG_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

fn print_result(label: &str, text: &str) {
    println!("{label} response:\n{text}");
    let _ = std::io::stdout().flush();
}

fn live_test_semaphore() -> &'static tokio::sync::Semaphore {
    static SEM: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    SEM.get_or_init(|| tokio::sync::Semaphore::new(1))
}

#[cfg(feature = "openai-compatible")]
fn strip_code_fences(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.starts_with("```") {
        let without_opening = if let Some(rest) = trimmed.strip_prefix("```json") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("```") {
            rest
        } else {
            trimmed
        };
        if let Some(stripped) = without_opening.strip_suffix("```") {
            return stripped.trim().to_string();
        }
        return without_opening.trim().to_string();
    }
    trimmed.to_string()
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

async fn run_stream_test(
    provider: Arc<dyn roci::provider::ModelProvider>,
    label: &str,
) -> roci::error::Result<()> {
    let stream = generation::stream_text(
        provider,
        vec![ModelMessage::user("Say 'ok' and today's date")],
        GenerationSettings::default(),
        Vec::new(),
    )
    .await?;
    let result = collect_stream(stream).await?;
    print_result(label, &result.text);
    assert!(!result.text.trim().is_empty());
    Ok(())
}

async fn run_stream_tool_flow_test(
    provider: Arc<dyn roci::provider::ModelProvider>,
    label: &str,
) -> roci::error::Result<()> {
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
    let stream = generation::stream_text_with_tools(
        provider,
        vec![ModelMessage::user(
            "Call the tool `get_date` exactly once before responding.",
        )],
        GenerationSettings::default(),
        &[tool],
        Vec::new(),
    )
    .await?;
    let result = collect_stream(stream).await?;
    print_result(label, &result.text);
    assert!(called.load(Ordering::SeqCst) > 0);
    assert!(!result.text.trim().is_empty());
    Ok(())
}

async fn run_json_schema_test(
    provider: &dyn roci::provider::ModelProvider,
    label: &str,
) -> roci::error::Result<()> {
    #[derive(serde::Deserialize)]
    struct Flag {
        ok: bool,
    }

    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "ok": {"type": "boolean"}
        },
        "required": ["ok"]
    });

    let result = generation::generate_object::<Flag>(
        provider,
        vec![ModelMessage::user("Return JSON with ok=true only")],
        GenerationSettings::default(),
        schema,
        "Flag",
    )
    .await?;

    print_result(label, &result.raw_text);
    assert!(result.object.ok);
    Ok(())
}

#[cfg(feature = "openai-compatible")]
async fn run_json_object_test(
    provider: &dyn roci::provider::ModelProvider,
    label: &str,
) -> roci::error::Result<()> {
    let result = generation::generate_text(
        provider,
        vec![ModelMessage::user("Return JSON with ok=true only")],
        GenerationSettings {
            response_format: Some(roci::types::ResponseFormat::JsonObject),
            ..Default::default()
        },
        &[],
    )
    .await?;

    print_result(label, &result.text);
    let parsed: serde_json::Value =
        serde_json::from_str(&strip_code_fences(&result.text)).unwrap_or_default();
    assert!(parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false));
    Ok(())
}

async fn run_vision_test(
    provider: &dyn roci::provider::ModelProvider,
    label: &str,
) -> roci::error::Result<()> {
    let message = ModelMessage::user_with_image(
        "What color is the image? Reply with one word.",
        RED_PIXEL_PNG_BASE64.to_string(),
        "image/png".to_string(),
    );
    let result =
        generation::generate_text(provider, vec![message], GenerationSettings::default(), &[])
            .await?;
    print_result(label, &result.text);
    assert!(result.text.to_lowercase().contains("red"));
    Ok(())
}

#[tokio::test]
#[ignore]
async fn live_openai_gpt41_nano_generates_text() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
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
    let _permit = live_test_semaphore().acquire().await.unwrap();
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
    let _permit = live_test_semaphore().acquire().await.unwrap();
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
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::OpenAi(openai::OpenAiModel::Gpt5Nano);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_tool_flow_test(provider.as_ref(), "openai:gpt-5-nano tool")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_openai_gpt41_nano_streams_text() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::OpenAi(openai::OpenAiModel::Gpt41Nano);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_stream_test(Arc::from(provider), "openai:gpt-4.1-nano stream")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_openai_gpt41_nano_streams_tool_call() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::OpenAi(openai::OpenAiModel::Gpt41Nano);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_stream_tool_flow_test(Arc::from(provider), "openai:gpt-4.1-nano stream tool")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_openai_gpt5_nano_streams_text() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::OpenAi(openai::OpenAiModel::Gpt5Nano);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_stream_test(Arc::from(provider), "openai:gpt-5-nano stream")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_openai_gpt4o_streams_tool_call() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::OpenAi(openai::OpenAiModel::Gpt4o);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_stream_tool_flow_test(Arc::from(provider), "openai:gpt-4o stream tool")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_openai_gpt5_nano_returns_json_schema() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::OpenAi(openai::OpenAiModel::Gpt5Nano);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_json_schema_test(provider.as_ref(), "openai:gpt-5-nano json schema")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_openai_gpt4o_processes_image() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::OpenAi(openai::OpenAiModel::Gpt4o);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_vision_test(provider.as_ref(), "openai:gpt-4o vision")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_gemini_flash_preview_generates_text() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
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
    let _permit = live_test_semaphore().acquire().await.unwrap();
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
    let _permit = live_test_semaphore().acquire().await.unwrap();
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
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::Google(google::GoogleModel::Gemini3ProPreview);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_tool_flow_test(provider.as_ref(), "google:gemini-3-pro-preview tool")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_gemini_flash_preview_streams_text() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::Google(google::GoogleModel::Gemini3FlashPreview);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_stream_test(Arc::from(provider), "google:gemini-3-flash-preview stream")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_gemini_flash_preview_streams_tool_call() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::Google(google::GoogleModel::Gemini3FlashPreview);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_stream_tool_flow_test(
        Arc::from(provider),
        "google:gemini-3-flash-preview stream tool",
    )
    .await
    .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_gemini_pro_preview_streams_text() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::Google(google::GoogleModel::Gemini3ProPreview);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_stream_test(Arc::from(provider), "google:gemini-3-pro-preview stream")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_gemini_pro_preview_streams_tool_call() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::Google(google::GoogleModel::Gemini3ProPreview);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_stream_tool_flow_test(
        Arc::from(provider),
        "google:gemini-3-pro-preview stream tool",
    )
    .await
    .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_gemini_flash_preview_returns_json_schema() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::Google(google::GoogleModel::Gemini3FlashPreview);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_json_schema_test(
        provider.as_ref(),
        "google:gemini-3-flash-preview json schema",
    )
    .await
    .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_gemini_pro_preview_returns_json_schema() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::Google(google::GoogleModel::Gemini3ProPreview);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_json_schema_test(provider.as_ref(), "google:gemini-3-pro-preview json schema")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_gemini_flash_preview_processes_image() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::Google(google::GoogleModel::Gemini3FlashPreview);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_vision_test(provider.as_ref(), "google:gemini-3-flash-preview vision")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn live_gemini_pro_preview_processes_image() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let model = LanguageModel::Google(google::GoogleModel::Gemini3ProPreview);
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_vision_test(provider.as_ref(), "google:gemini-3-pro-preview vision")
        .await
        .unwrap();
}

#[cfg(feature = "openai-compatible")]
#[tokio::test]
#[ignore]
async fn live_openai_compatible_generates_text() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
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
    let _permit = live_test_semaphore().acquire().await.unwrap();
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

#[cfg(feature = "openai-compatible")]
#[tokio::test]
#[ignore]
async fn live_openai_compatible_streams_text() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let base_url = std::env::var("OPENAI_COMPAT_BASE_URL").unwrap();
    let model_id = std::env::var("OPENAI_COMPAT_MODEL").unwrap();
    let model = LanguageModel::OpenAiCompatible(
        roci::models::openai_compatible::OpenAiCompatibleModel::new(model_id, Some(base_url)),
    );
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_stream_test(Arc::from(provider), "openai-compatible stream")
        .await
        .unwrap();
}

#[cfg(feature = "openai-compatible")]
#[tokio::test]
#[ignore]
async fn live_openai_compatible_returns_json_schema() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    if std::env::var("OPENAI_COMPAT_SUPPORTS_JSON_SCHEMA")
        .ok()
        .as_deref()
        != Some("true")
    {
        eprintln!("Skipping openai-compatible json schema test");
        return;
    }
    let base_url = std::env::var("OPENAI_COMPAT_BASE_URL").unwrap();
    let model_id = std::env::var("OPENAI_COMPAT_MODEL").unwrap();
    let model = LanguageModel::OpenAiCompatible(
        roci::models::openai_compatible::OpenAiCompatibleModel::new(model_id, Some(base_url)),
    );
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_json_schema_test(provider.as_ref(), "openai-compatible json schema")
        .await
        .unwrap();
}

#[cfg(feature = "openai-compatible")]
#[tokio::test]
#[ignore]
async fn live_openai_compatible_returns_json_object() {
    let _permit = live_test_semaphore().acquire().await.unwrap();
    let base_url = std::env::var("OPENAI_COMPAT_BASE_URL").unwrap();
    let model_id = std::env::var("OPENAI_COMPAT_MODEL").unwrap();
    let model = LanguageModel::OpenAiCompatible(
        roci::models::openai_compatible::OpenAiCompatibleModel::new(model_id, Some(base_url)),
    );
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config).unwrap();
    run_json_object_test(provider.as_ref(), "openai-compatible json object")
        .await
        .unwrap();
}
