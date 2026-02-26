//! Validate tool call arguments against JSON Schema before execution.

/// Validate tool arguments against a JSON Schema.
///
/// Performs top-level validation: schema type check, required field presence,
/// and property type verification. Returns `Ok(())` when valid,
/// `Err(message)` describing the first violation found.
pub fn validate_arguments(
    args: &serde_json::Value,
    schema: &serde_json::Value,
) -> Result<(), String> {
    if let Some(schema_type) = schema.get("type").and_then(|v| v.as_str()) {
        if schema_type == "object" && !args.is_object() {
            return Err(format!(
                "expected object arguments, got {}",
                json_type_name(args)
            ));
        }
    }

    if let Some(required) = schema.get("required").and_then(|v| v.as_array()) {
        let obj = match args.as_object() {
            Some(obj) => obj,
            None => return Ok(()),
        };
        for field in required {
            if let Some(name) = field.as_str() {
                if !obj.contains_key(name) {
                    return Err(format!("missing required field '{name}'"));
                }
            }
        }
    }

    if let (Some(properties), Some(obj)) = (
        schema.get("properties").and_then(|v| v.as_object()),
        args.as_object(),
    ) {
        for (key, value) in obj {
            if let Some(prop_schema) = properties.get(key) {
                if let Some(expected_type) = prop_schema.get("type").and_then(|v| v.as_str()) {
                    if !value_matches_type(value, expected_type) {
                        return Err(format!(
                            "field '{}' expected type '{}', got {}",
                            key,
                            expected_type,
                            json_type_name(value)
                        ));
                    }
                }
            }
        }
    }

    Ok(())
}

fn value_matches_type(value: &serde_json::Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}
