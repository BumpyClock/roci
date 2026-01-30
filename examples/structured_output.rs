//! Structured output example: generate a typed object.

use roci::prelude::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Recipe {
    name: String,
    ingredients: Vec<String>,
    steps: Vec<String>,
    prep_time_minutes: u32,
}

#[tokio::main]
async fn main() -> roci::error::Result<()> {
    let model: LanguageModel = "openai:gpt-4o".parse()?;
    let config = RociConfig::from_env();
    let provider = roci::provider::create_provider(&model, &config)?;

    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "ingredients": {"type": "array", "items": {"type": "string"}},
            "steps": {"type": "array", "items": {"type": "string"}},
            "prep_time_minutes": {"type": "integer"}
        },
        "required": ["name", "ingredients", "steps", "prep_time_minutes"]
    });

    let result = roci::generation::generate_object::<Recipe>(
        provider.as_ref(),
        vec![ModelMessage::user("Give me a simple pasta recipe")],
        GenerationSettings::default(),
        schema,
        "Recipe",
    )
    .await?;

    println!("Recipe: {}", result.object.name);
    println!("Prep time: {} min", result.object.prep_time_minutes);
    println!("Ingredients:");
    for i in &result.object.ingredients {
        println!("  - {i}");
    }
    println!("Steps:");
    for (n, s) in result.object.steps.iter().enumerate() {
        println!("  {}. {s}", n + 1);
    }

    Ok(())
}
