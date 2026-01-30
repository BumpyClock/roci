//! MCP schema types.

use serde::{Deserialize, Serialize};

/// Schema for a tool exposed by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPToolSchema {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

/// Builder for constructing MCP-compatible JSON schemas.
pub struct SchemaBuilder {
    properties: serde_json::Map<String, serde_json::Value>,
    required: Vec<String>,
    description: Option<String>,
}

impl SchemaBuilder {
    pub fn new() -> Self {
        Self {
            properties: serde_json::Map::new(),
            required: Vec::new(),
            description: None,
        }
    }

    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn property(
        mut self,
        name: impl Into<String>,
        schema: serde_json::Value,
        required: bool,
    ) -> Self {
        let name = name.into();
        self.properties.insert(name.clone(), schema);
        if required {
            self.required.push(name);
        }
        self
    }

    pub fn build(self) -> serde_json::Value {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": self.properties,
        });
        if !self.required.is_empty() {
            schema["required"] = serde_json::json!(self.required);
        }
        if let Some(desc) = self.description {
            schema["description"] = serde_json::Value::String(desc);
        }
        schema
    }
}

impl Default for SchemaBuilder {
    fn default() -> Self {
        Self::new()
    }
}
