//! Structured output: generate typed objects from model responses.

use serde::de::DeserializeOwned;

use crate::error::RociError;
use crate::provider::ModelProvider;
use crate::types::*;

/// Generate a typed object by asking the model to produce JSON.
///
/// Uses JSON Schema response format if the model supports it,
/// otherwise uses system prompt instructions.
pub async fn generate_object<T: DeserializeOwned>(
    provider: &dyn ModelProvider,
    mut messages: Vec<ModelMessage>,
    settings: GenerationSettings,
    schema: serde_json::Value,
    type_name: &str,
) -> Result<GenerateObjectResult<T>, RociError> {
    let supports_json_schema = provider.capabilities().supports_json_schema;
    let supports_json_mode = provider.capabilities().supports_json_mode;

    let mut settings = settings;

    if supports_json_schema {
        settings.response_format = Some(ResponseFormat::JsonSchema {
            schema: schema.clone(),
            name: type_name.to_string(),
        });
    } else if supports_json_mode {
        settings.response_format = Some(ResponseFormat::JsonObject);
        // Prepend schema instruction to system message
        let schema_instruction = format!(
            "You must respond with valid JSON matching this schema:\n```json\n{}\n```",
            serde_json::to_string_pretty(&schema).unwrap_or_default()
        );
        messages.insert(0, ModelMessage::system(schema_instruction));
    } else {
        // Fallback: instruct via system message
        let schema_instruction = format!(
            "You must respond with ONLY valid JSON (no markdown, no explanation) matching this schema:\n```json\n{}\n```",
            serde_json::to_string_pretty(&schema).unwrap_or_default()
        );
        messages.insert(0, ModelMessage::system(schema_instruction));
    }

    let result = super::text::generate_text(provider, messages, settings, &[]).await?;

    // Parse the JSON from the response
    let raw_text = result.text.trim().to_string();
    // Strip potential markdown code fences
    let json_text = strip_code_fences(&raw_text);

    let object: T = serde_json::from_str(&json_text).map_err(|e| {
        RociError::Serialization(e)
    })?;

    Ok(GenerateObjectResult {
        object,
        raw_text,
        usage: result.usage,
        finish_reason: result.finish_reason,
    })
}

/// Generate a typed object via streaming (collects full response, then parses).
pub async fn stream_object<T: DeserializeOwned>(
    provider: &dyn ModelProvider,
    messages: Vec<ModelMessage>,
    settings: GenerationSettings,
    schema: serde_json::Value,
    type_name: &str,
) -> Result<GenerateObjectResult<T>, RociError> {
    // For structured output, streaming doesn't add much value since
    // we need the full JSON before parsing. Delegate to non-streaming.
    generate_object(provider, messages, settings, schema, type_name).await
}

/// Strip markdown code fences from JSON response.
fn strip_code_fences(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.starts_with("```") {
        let without_opening = if let Some(rest) = trimmed.strip_prefix("```json") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("```") {
            rest
        } else {
            trimmed
        };
        if let Some(stripped) = without_opening.strip_suffix("```") {
            return stripped.trim().to_string();
        }
        return without_opening.trim().to_string();
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_code_fences_plain_json() {
        assert_eq!(strip_code_fences(r#"{"key": "value"}"#), r#"{"key": "value"}"#);
    }

    #[test]
    fn strip_code_fences_with_json_fence() {
        let input = "```json\n{\"key\": \"value\"}\n```";
        assert_eq!(strip_code_fences(input), r#"{"key": "value"}"#);
    }

    #[test]
    fn strip_code_fences_with_bare_fence() {
        let input = "```\n{\"key\": \"value\"}\n```";
        assert_eq!(strip_code_fences(input), r#"{"key": "value"}"#);
    }
}
