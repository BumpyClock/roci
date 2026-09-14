//! Schema normalization for provider-specific structured output.

use serde_json::Value;

/// Normalize a JSON schema for a specific provider.
pub fn normalize_schema_for_provider(schema: &Value, provider_name: &str) -> Value {
    match provider_name {
        "openai" | "openai-compatible" => ensure_additional_properties_false(schema),
        "google" => strip_additional_properties(schema),
        _ => schema.clone(),
    }
}

fn ensure_additional_properties_false(schema: &Value) -> Value {
    match schema {
        Value::Object(obj) => {
            let mut normalized = serde_json::Map::new();
            for (key, value) in obj {
                let next = match key.as_str() {
                    "properties" => ensure_properties_additional_false(value),
                    _ => ensure_additional_properties_false(value),
                };
                normalized.insert(key.clone(), next);
            }
            if is_object_schema(schema) {
                normalized
                    .entry("additionalProperties")
                    .or_insert(Value::Bool(false));
            }
            Value::Object(normalized)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(ensure_additional_properties_false)
                .collect(),
        ),
        _ => schema.clone(),
    }
}

fn ensure_properties_additional_false(schema: &Value) -> Value {
    if let Value::Object(properties) = schema {
        let mut normalized = serde_json::Map::new();
        for (key, value) in properties {
            normalized.insert(key.clone(), ensure_additional_properties_false(value));
        }
        Value::Object(normalized)
    } else {
        ensure_additional_properties_false(schema)
    }
}

fn strip_additional_properties(schema: &Value) -> Value {
    match schema {
        Value::Object(obj) => {
            let mut normalized = serde_json::Map::new();
            for (key, value) in obj {
                if key == "additionalProperties" {
                    continue;
                }
                normalized.insert(key.clone(), strip_additional_properties(value));
            }
            Value::Object(normalized)
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(strip_additional_properties).collect())
        }
        _ => schema.clone(),
    }
}

fn is_object_schema(value: &Value) -> bool {
    if let Value::Object(obj) = value {
        matches!(obj.get("type"), Some(Value::String(t)) if t == "object")
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_schema_adds_additional_properties_for_openai() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "ok": {"type": "boolean"},
                "properties": {
                    "type": "object",
                    "properties": {"nested": {"type": "object"}}
                },
                "rows": {"type": "array", "items": {"type": "object"}},
                "open": {"type": "object", "additionalProperties": true}
            },
            "required": ["ok"]
        });
        let expected = serde_json::json!({
            "type": "object",
            "properties": {
                "ok": {"type": "boolean"},
                "properties": {
                    "type": "object",
                    "properties": {
                        "nested": {"type": "object", "additionalProperties": false}
                    },
                    "additionalProperties": false
                },
                "rows": {
                    "type": "array",
                    "items": {"type": "object", "additionalProperties": false}
                },
                "open": {"type": "object", "additionalProperties": true}
            },
            "required": ["ok"],
            "additionalProperties": false
        });
        for provider in ["openai", "openai-compatible"] {
            assert_eq!(normalize_schema_for_provider(&schema, provider), expected);
        }
    }

    #[test]
    fn normalize_schema_strips_additional_properties_for_google() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "ok": {"type": "boolean"},
                "rows": {
                    "type": "array",
                    "items": {"type": "object", "additionalProperties": true}
                },
                "choice": {"anyOf": [
                    {"type": "object", "additionalProperties": false},
                    {"type": "null"}
                ]}
            },
            "required": ["ok"],
            "additionalProperties": false
        });
        let normalized = normalize_schema_for_provider(&schema, "google");
        assert_eq!(
            normalized,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "ok": {"type": "boolean"},
                    "rows": {"type": "array", "items": {"type": "object"}},
                    "choice": {"anyOf": [{"type": "object"}, {"type": "null"}]}
                },
                "required": ["ok"]
            })
        );
    }
}
