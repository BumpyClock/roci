//! Streaming generation example.

use std::sync::Arc;

use futures::StreamExt;
use roci::prelude::*;

#[tokio::main]
async fn main() -> roci::error::Result<()> {
    let model: LanguageModel = "openai:gpt-4o".parse()?;
    let config = RociConfig::from_env();
    let registry = roci::default_registry();
    let provider: Arc<dyn ModelProvider> =
        Arc::from(registry.create_provider(model.provider_name(), model.model_id(), &config)?);

    let mut stream = roci::generation::stream(provider, "Write a haiku about Rust.").await?;

    while let Some(delta) = stream.next().await {
        let delta = delta?;
        print!("{}", delta.text);
    }
    println!();

    Ok(())
}
