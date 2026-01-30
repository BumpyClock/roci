//! Tool-related types: parameter schemas, calls, results.

use serde::{Deserialize, Serialize};

/// JSON Schema-based parameter definition for a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolParameters {
    /// JSON Schema object describing the parameters.
    pub schema: serde_json::Value,
}

impl AgentToolParameters {
    /// Create from a raw JSON Schema value.
    pub fn from_schema(schema: serde_json::Value) -> Self {
        Self { schema }
    }

    /// Create an empty parameter schema (no parameters).
    pub fn empty() -> Self {
        Self {
            schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": [],
            }),
        }
    }

    /// Builder: create an object schema with properties.
    pub fn object() -> ParameterBuilder {
        ParameterBuilder {
            properties: serde_json::Map::new(),
            required: Vec::new(),
        }
    }
}

/// Builder for constructing tool parameter schemas.
pub struct ParameterBuilder {
    properties: serde_json::Map<String, serde_json::Value>,
    required: Vec<String>,
}

impl ParameterBuilder {
    /// Add a string property.
    pub fn string(mut self, name: impl Into<String>, description: impl Into<String>, required: bool) -> Self {
        let name = name.into();
        self.properties.insert(
            name.clone(),
            serde_json::json!({
                "type": "string",
                "description": description.into(),
            }),
        );
        if required {
            self.required.push(name);
        }
        self
    }

    /// Add a number property.
    pub fn number(mut self, name: impl Into<String>, description: impl Into<String>, required: bool) -> Self {
        let name = name.into();
        self.properties.insert(
            name.clone(),
            serde_json::json!({
                "type": "number",
                "description": description.into(),
            }),
        );
        if required {
            self.required.push(name);
        }
        self
    }

    /// Add a boolean property.
    pub fn boolean(mut self, name: impl Into<String>, description: impl Into<String>, required: bool) -> Self {
        let name = name.into();
        self.properties.insert(
            name.clone(),
            serde_json::json!({
                "type": "boolean",
                "description": description.into(),
            }),
        );
        if required {
            self.required.push(name);
        }
        self
    }

    /// Add an enum (string) property.
    pub fn string_enum(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        values: &[&str],
        required: bool,
    ) -> Self {
        let name = name.into();
        self.properties.insert(
            name.clone(),
            serde_json::json!({
                "type": "string",
                "description": description.into(),
                "enum": values,
            }),
        );
        if required {
            self.required.push(name);
        }
        self
    }

    /// Build into AgentToolParameters.
    pub fn build(self) -> AgentToolParameters {
        AgentToolParameters {
            schema: serde_json::json!({
                "type": "object",
                "properties": self.properties,
                "required": self.required,
            }),
        }
    }
}
