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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_non_object_args_when_schema_expects_object() {
        let schema = json!({ "type": "object", "properties": {}, "required": [] });
        let args = json!("not an object");

        let result = validate_arguments(&args, &schema);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected object"));
    }

    #[test]
    fn rejects_missing_required_field() {
        let schema = json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
        });
        let args = json!({});

        let result = validate_arguments(&args, &schema);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("missing required field 'path'"));
    }

    #[test]
    fn rejects_when_any_required_field_is_absent() {
        let schema = json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" },
            },
            "required": ["path", "content"],
        });
        let args = json!({ "path": "test.txt" });

        let result = validate_arguments(&args, &schema);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("missing required field 'content'"));
    }

    #[test]
    fn accepts_valid_args_with_all_required_fields() {
        let schema = json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
        });
        let args = json!({ "path": "test.txt" });

        let result = validate_arguments(&args, &schema);

        assert!(result.is_ok());
    }

    #[test]
    fn accepts_any_args_when_schema_has_no_required_fields() {
        let schema = json!({
            "type": "object",
            "properties": { "verbose": { "type": "boolean" } },
            "required": [],
        });
        let args = json!({});

        let result = validate_arguments(&args, &schema);

        assert!(result.is_ok());
    }

    #[test]
    fn accepts_any_args_when_schema_is_empty_object() {
        let schema = json!({});
        let args = json!({ "anything": 42 });

        let result = validate_arguments(&args, &schema);

        assert!(result.is_ok());
    }

    #[test]
    fn rejects_field_with_wrong_type() {
        let schema = json!({
            "type": "object",
            "properties": { "count": { "type": "integer" } },
            "required": ["count"],
        });
        let args = json!({ "count": "not a number" });

        let result = validate_arguments(&args, &schema);

        let err = result.unwrap_err();
        assert!(err.contains("field 'count'"));
        assert!(err.contains("expected type 'integer'"));
    }

    #[test]
    fn accepts_extra_fields_not_in_schema_properties() {
        let schema = json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
        });
        let args = json!({ "path": "test.txt", "extra": true });

        let result = validate_arguments(&args, &schema);

        assert!(result.is_ok());
    }

    #[test]
    fn rejects_number_where_string_expected() {
        let schema = json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"],
        });
        let args = json!({ "name": 42 });

        let result = validate_arguments(&args, &schema);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected type 'string'"));
    }

    #[test]
    fn accepts_optional_field_when_absent() {
        let schema = json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "verbose": { "type": "boolean" },
            },
            "required": ["path"],
        });
        let args = json!({ "path": "test.txt" });

        let result = validate_arguments(&args, &schema);

        assert!(result.is_ok());
    }

    #[test]
    fn validates_boolean_type_correctly() {
        let schema = json!({
            "type": "object",
            "properties": { "flag": { "type": "boolean" } },
            "required": ["flag"],
        });

        assert!(validate_arguments(&json!({ "flag": true }), &schema).is_ok());
        assert!(validate_arguments(&json!({ "flag": "yes" }), &schema).is_err());
    }

    #[test]
    fn validates_array_type_correctly() {
        let schema = json!({
            "type": "object",
            "properties": { "items": { "type": "array" } },
            "required": ["items"],
        });

        assert!(validate_arguments(&json!({ "items": [1, 2] }), &schema).is_ok());
        assert!(validate_arguments(&json!({ "items": "not array" }), &schema).is_err());
    }

    #[test]
    fn accepts_null_args_when_schema_has_no_type() {
        let schema = json!({});
        let args = serde_json::Value::Null;

        let result = validate_arguments(&args, &schema);

        assert!(result.is_ok());
    }
}
