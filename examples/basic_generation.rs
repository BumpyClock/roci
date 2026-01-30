//! Basic text generation example.

use roci::prelude::*;

#[tokio::main]
async fn main() -> roci::error::Result<()> {
    let model: LanguageModel = "openai:gpt-4o".parse()?;
    let response = roci::generation::generate(&model, "What is Rust?").await?;
    println!("{response}");
    Ok(())
}
