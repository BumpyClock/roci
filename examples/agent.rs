//! Agent example with conversation and tools.

use std::sync::Arc;

use roci::prelude::*;

#[tokio::main]
#[allow(clippy::result_large_err)]
async fn main() -> roci::error::Result<()> {
    let model: LanguageModel = "openai:gpt-4o".parse()?;
    let registry = Arc::new(roci::default_registry());

    let calc_tool: Box<dyn Tool> = Box::new(AgentTool::new(
        "calculate",
        "Evaluate a math expression",
        AgentToolParameters::object()
            .string("expression", "Math expression to evaluate", true)
            .build(),
        |args, _ctx| async move {
            let expr = args.get_str("expression")?;
            // Simplified: just return the expression (real impl would eval)
            Ok(serde_json::json!({"result": expr, "note": "evaluation stub"}))
        },
    ));

    let mut agent = roci::agent::Agent::new(model, registry)
        .with_system_prompt("You are a helpful math assistant.")
        .with_tool(calc_tool);

    let response = agent.execute("What is 2 + 2?").await?;
    println!("Agent: {response}");

    let response = agent.execute("And what about 3 * 7?").await?;
    println!("Agent: {response}");

    println!(
        "\nConversation length: {} messages",
        agent.conversation().len()
    );

    Ok(())
}
