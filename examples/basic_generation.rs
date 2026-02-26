//! Basic text generation example.

use roci::prelude::*;

#[tokio::main]
async fn main() -> roci::error::Result<()> {
    let model: LanguageModel = "openai:gpt-4o".parse()?;
    let config = RociConfig::from_env();
    let registry = roci::default_registry();
    let provider = registry.create_provider(model.provider_name(), model.model_id(), &config)?;

    let response = roci::generation::generate(provider.as_ref(), "What is Rust?").await?;
    println!("{response}");
    Ok(())
}
