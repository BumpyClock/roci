//! Shared error mapping for OpenAI API responses.
//!
//! Used by both OpenAI Chat and OpenAI Responses providers.

use roci_core::error::{ErrorCode, ErrorDetails, RociError};

pub(crate) fn map_openai_error_code(code: &str) -> ErrorCode {
    match code {
        "invalid_api_key" => ErrorCode::InvalidApiKey,
        "insufficient_quota" => ErrorCode::InsufficientQuota,
        "rate_limit_exceeded" => ErrorCode::RateLimitExceeded,
        "model_not_found" => ErrorCode::ModelNotFound,
        "invalid_request_error" => ErrorCode::InvalidRequest,
        "context_length_exceeded" => ErrorCode::ContextLengthExceeded,
        "content_filter" => ErrorCode::ContentFiltered,
        "server_error" => ErrorCode::ServerError,
        "service_unavailable" => ErrorCode::ServiceUnavailable,
        "timeout" => ErrorCode::Timeout,
        _ => ErrorCode::Unknown,
    }
}

pub(crate) fn parse_openai_error_details(body: &str) -> Option<(String, ErrorDetails)> {
    let payload = serde_json::from_str::<serde_json::Value>(body).ok()?;
    let error = payload.get("error")?;
    let provider_code = error
        .get("code")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let code = provider_code.as_deref().map(map_openai_error_code);
    let message = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(body)
        .to_string();
    let param = error
        .get("param")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let request_id = payload
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Some((
        message,
        ErrorDetails {
            code,
            provider_code,
            param,
            request_id,
        },
    ))
}

/// Map an HTTP status + response body to a structured [`RociError`].
///
/// Rate-limit errors (429) delegate to the generic handler so that
/// retry-after headers are honoured.  Everything else attempts
/// structured parsing first.
pub(crate) fn status_to_openai_error(status: u16, body: &str) -> RociError {
    if status == 429 {
        return roci_core::provider::http::status_to_error(status, body);
    }
    if let Some((message, details)) = parse_openai_error_details(body) {
        return RociError::api_with_details(status, message, details);
    }
    roci_core::provider::http::status_to_error(status, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_error_codes() {
        assert_eq!(
            map_openai_error_code("invalid_api_key"),
            ErrorCode::InvalidApiKey
        );
        assert_eq!(
            map_openai_error_code("context_length_exceeded"),
            ErrorCode::ContextLengthExceeded
        );
        assert_eq!(map_openai_error_code("unknown_code"), ErrorCode::Unknown);
    }

    #[test]
    fn parses_structured_error_details() {
        let body = serde_json::json!({
            "error": {
                "message": "Bad request",
                "code": "invalid_request_error",
                "param": "input"
            }
        })
        .to_string();

        let (message, details) = parse_openai_error_details(&body).unwrap();
        assert_eq!(message, "Bad request");
        assert_eq!(details.code, Some(ErrorCode::InvalidRequest));
        assert_eq!(details.param.as_deref(), Some("input"));
    }

    #[test]
    fn returns_none_for_non_json_body() {
        assert!(parse_openai_error_details("not json").is_none());
    }

    #[test]
    fn returns_none_for_missing_error_key() {
        let body = serde_json::json!({"ok": true}).to_string();
        assert!(parse_openai_error_details(&body).is_none());
    }
}
