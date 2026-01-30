//! Streaming generation example.

use futures::StreamExt;
use roci::prelude::*;

#[tokio::main]
async fn main() -> roci::error::Result<()> {
    let model: LanguageModel = "openai:gpt-4o".parse()?;
    let mut stream = roci::generation::stream(&model, "Write a haiku about Rust.").await?;

    while let Some(delta) = stream.next().await {
        let delta = delta?;
        print!("{}", delta.text);
    }
    println!();

    Ok(())
}
