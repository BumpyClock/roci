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
            let mut prop = ensure_additional_properties_false(value);
            if is_object_schema(value) {
                if let Value::Object(prop_obj) = &mut prop {
                    prop_obj
                        .entry("additionalProperties")
                        .or_insert(Value::Bool(false));
                }
            }
            normalized.insert(key.clone(), prop);
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
                "ok": {"type": "boolean"}
            },
            "required": ["ok"]
        });
        let normalized = normalize_schema_for_provider(&schema, "openai");
        assert_eq!(normalized["additionalProperties"], false);
    }

    #[test]
    fn normalize_schema_strips_additional_properties_for_google() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "ok": {"type": "boolean"}
            },
            "required": ["ok"],
            "additionalProperties": false
        });
        let normalized = normalize_schema_for_provider(&schema, "google");
        assert!(normalized.get("additionalProperties").is_none());
    }
}
