//! Multi-provider example: same prompt, different models.

use roci::prelude::*;

#[tokio::main]
#[allow(clippy::result_large_err)]
async fn main() -> roci::error::Result<()> {
    let models = vec![
        "openai:gpt-4o",
        "anthropic:claude-sonnet-4-5-20250514",
        "google:gemini-2.5-flash",
    ];

    let config = RociConfig::from_env();
    let registry = roci::default_registry();
    let prompt = "In one sentence, what makes Rust unique?";

    for model_str in models {
        let model: LanguageModel = match model_str.parse() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Skip {model_str}: {e}");
                continue;
            }
        };

        let provider =
            match registry.create_provider(model.provider_name(), model.model_id(), &config) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Skip {model_str}: {e}");
                    continue;
                }
            };

        match roci::generation::generate_text(
            provider.as_ref(),
            vec![ModelMessage::user(prompt)],
            GenerationSettings::default(),
            &[],
        )
        .await
        {
            Ok(result) => {
                println!("[{model_str}]: {}", result.text);
                println!(
                    "  tokens: {} in / {} out",
                    result.usage.input_tokens, result.usage.output_tokens
                );
            }
            Err(e) => eprintln!("[{model_str}]: Error: {e}"),
        }
        println!();
    }

    Ok(())
}
