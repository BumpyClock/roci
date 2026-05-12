//! Tool calling example.

#[cfg(feature = "agent")]
use std::sync::Arc;

#[cfg(feature = "agent")]
use roci::prelude::*;

#[cfg(feature = "agent")]
#[tokio::main]
#[allow(clippy::result_large_err)]
async fn main() -> roci::error::Result<()> {
    let model: LanguageModel = "openai:gpt-4o".parse()?;
    let registry = Arc::new(roci::default_registry());

    let weather_tool: Box<dyn Tool> = Box::new(
        AgentTool::new(
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
        )
        .with_static_safety(
            roci::tools::ToolSafetyPlan::safe_read_only(roci::tools::ToolSafetyKind::CustomTool),
            roci::tools::ToolSafetySummary {
                read_only_by_default: true,
                destructive_by_default: false,
                concurrency_safe_by_default: true,
                approval_kind: roci::tools::ToolSafetyKind::CustomTool,
            },
        ),
    );

    let mut agent = roci::agent::Agent::new(model, registry).with_tool(weather_tool);
    let result = agent.execute("What's the weather in Tokyo?").await?;

    println!("{result}");

    Ok(())
}

#[cfg(not(feature = "agent"))]
fn main() {
    eprintln!("tool_calling example requires the `agent` feature");
    std::process::exit(1);
}
