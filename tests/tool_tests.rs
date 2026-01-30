//! Tests for the tool system.

use roci::tools::tool::{AgentTool, Tool, ToolExecutionContext};
use roci::tools::*;

#[test]
fn parameter_builder_constructs_schema() {
    let params = AgentToolParameters::object()
        .string("query", "Search query", true)
        .number("limit", "Max results", false)
        .boolean("verbose", "Enable verbose output", false)
        .build();

    let schema = &params.schema;
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["query"]["type"], "string");
    assert_eq!(schema["properties"]["limit"]["type"], "number");
    assert_eq!(schema["required"].as_array().unwrap().len(), 1);
}

#[test]
fn parameter_builder_string_enum() {
    let params = AgentToolParameters::object()
        .string_enum("format", "Output format", &["json", "text", "csv"], true)
        .build();

    let enums = params.schema["properties"]["format"]["enum"]
        .as_array()
        .unwrap();
    assert_eq!(enums.len(), 3);
}

#[test]
fn empty_parameters() {
    let params = AgentToolParameters::empty();
    assert_eq!(params.schema["type"], "object");
}

#[test]
fn tool_arguments_get_str() {
    let args = ToolArguments::new(serde_json::json!({"name": "Alice", "age": 30}));
    assert_eq!(args.get_str("name").unwrap(), "Alice");
    assert!(args.get_str("missing").is_err());
}

#[test]
fn tool_arguments_get_i64() {
    let args = ToolArguments::new(serde_json::json!({"count": 42}));
    assert_eq!(args.get_i64("count").unwrap(), 42);
}

#[test]
fn tool_arguments_get_bool() {
    let args = ToolArguments::new(serde_json::json!({"active": true}));
    assert!(args.get_bool("active").unwrap());
}

#[test]
fn tool_arguments_optional() {
    let args = ToolArguments::new(serde_json::json!({"name": "test"}));
    assert_eq!(args.get_str_opt("name"), Some("test"));
    assert_eq!(args.get_str_opt("missing"), None);
}

#[test]
fn tool_arguments_deserialize() {
    #[derive(serde::Deserialize, PartialEq, Debug)]
    struct Params {
        query: String,
        limit: Option<u32>,
    }

    let args = ToolArguments::new(serde_json::json!({"query": "rust", "limit": 10}));
    let params: Params = args.deserialize().unwrap();
    assert_eq!(params.query, "rust");
    assert_eq!(params.limit, Some(10));
}

#[tokio::test]
async fn agent_tool_executes() {
    let tool = AgentTool::new(
        "greet",
        "Greet a person",
        AgentToolParameters::object()
            .string("name", "Name", true)
            .build(),
        |args, _ctx| async move {
            let name = args.get_str("name")?;
            Ok(serde_json::json!({"greeting": format!("Hello, {}!", name)}))
        },
    );

    assert_eq!(tool.name(), "greet");
    assert_eq!(tool.description(), "Greet a person");

    let args = ToolArguments::new(serde_json::json!({"name": "World"}));
    let result = tool
        .execute(&args, &ToolExecutionContext::default())
        .await
        .unwrap();
    assert_eq!(result["greeting"], "Hello, World!");
}
