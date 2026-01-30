//! Tool calling example.

use roci::prelude::*;

#[tokio::main]
async fn main() -> roci::error::Result<()> {
    let model: LanguageModel = "openai:gpt-4o".parse()?;
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config)?;

    let weather_tool: std::sync::Arc<dyn Tool> = std::sync::Arc::new(AgentTool::new(
        "get_weather",
        "Get weather for a city",
        AgentToolParameters::object()
            .string("city", "City name", true)
            .build(),
        |args, _ctx| async move {
            let city = args.get_str("city")?;
            Ok(serde_json::json!({
                "city": city,
                "temperature": 22,
                "condition": "sunny"
            }))
        },
    ));

    let result = roci::generation::generate_text(
        provider.as_ref(),
        vec![ModelMessage::user("What's the weather in Tokyo?")],
        GenerationSettings::default(),
        &[weather_tool],
    )
    .await?;

    println!("{}", result.text);
    println!("Steps: {}", result.steps.len());
    println!(
        "Tokens: {} in / {} out",
        result.usage.input_tokens, result.usage.output_tokens
    );

    Ok(())
}
