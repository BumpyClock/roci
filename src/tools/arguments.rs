//! Typed access to tool call arguments.

use crate::error::RociError;

/// Wrapper around tool call arguments providing typed extraction.
#[derive(Debug, Clone)]
pub struct ToolArguments {
    value: serde_json::Value,
}

impl ToolArguments {
    pub fn new(value: serde_json::Value) -> Self {
        Self { value }
    }

    /// Get the raw JSON value.
    pub fn raw(&self) -> &serde_json::Value {
        &self.value
    }

    /// Get a string argument by key.
    pub fn get_str(&self, key: &str) -> Result<&str, RociError> {
        self.value
            .get(key)
            .and_then(|v| v.as_str())
            .ok_or_else(|| RociError::InvalidArgument(format!("Missing string argument: {key}")))
    }

    /// Get an optional string argument.
    pub fn get_str_opt(&self, key: &str) -> Option<&str> {
        self.value.get(key).and_then(|v| v.as_str())
    }

    /// Get an integer argument.
    pub fn get_i64(&self, key: &str) -> Result<i64, RociError> {
        self.value
            .get(key)
            .and_then(|v| v.as_i64())
            .ok_or_else(|| RociError::InvalidArgument(format!("Missing integer argument: {key}")))
    }

    /// Get a float argument.
    pub fn get_f64(&self, key: &str) -> Result<f64, RociError> {
        self.value
            .get(key)
            .and_then(|v| v.as_f64())
            .ok_or_else(|| RociError::InvalidArgument(format!("Missing float argument: {key}")))
    }

    /// Get a boolean argument.
    pub fn get_bool(&self, key: &str) -> Result<bool, RociError> {
        self.value
            .get(key)
            .and_then(|v| v.as_bool())
            .ok_or_else(|| RociError::InvalidArgument(format!("Missing boolean argument: {key}")))
    }

    /// Get a nested object.
    pub fn get_object(&self, key: &str) -> Result<&serde_json::Value, RociError> {
        self.value
            .get(key)
            .filter(|v| v.is_object())
            .ok_or_else(|| RociError::InvalidArgument(format!("Missing object argument: {key}")))
    }

    /// Get an array argument.
    pub fn get_array(&self, key: &str) -> Result<&Vec<serde_json::Value>, RociError> {
        self.value
            .get(key)
            .and_then(|v| v.as_array())
            .ok_or_else(|| RociError::InvalidArgument(format!("Missing array argument: {key}")))
    }

    /// Deserialize the entire arguments into a typed struct.
    pub fn deserialize<T: serde::de::DeserializeOwned>(&self) -> Result<T, RociError> {
        let value = match &self.value {
            serde_json::Value::String(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_str::<serde_json::Value>(trimmed).map_err(|e| {
                        RociError::InvalidArgument(format!("Failed to deserialize arguments: {e}"))
                    })?
                }
            }
            other => other.clone(),
        };
        serde_json::from_value(value).map_err(|e| {
            RociError::InvalidArgument(format!("Failed to deserialize arguments: {e}"))
        })
    }
}
