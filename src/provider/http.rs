//! Shared HTTP client, SSE parsing, and auth utilities.

use std::sync::OnceLock;

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};

use crate::error::RociError;

static SHARED_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Get (or create) the shared reqwest client.
pub fn shared_client() -> &'static reqwest::Client {
    SHARED_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .pool_max_idle_per_host(10)
            .build()
            .expect("Failed to build HTTP client")
    })
}

/// Build default headers for a Bearer-token API.
pub fn bearer_headers(api_key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Ok(val) = HeaderValue::from_str(&format!("Bearer {api_key}")) {
        headers.insert(AUTHORIZATION, val);
    }
    headers
}

/// Build Anthropic-style headers (x-api-key).
pub fn anthropic_headers(api_key: &str, version: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Ok(val) = HeaderValue::from_str(api_key) {
        headers.insert("x-api-key", val);
    }
    if let Ok(val) = HeaderValue::from_str(version) {
        headers.insert("anthropic-version", val);
    }
    headers
}

/// Parse an SSE "data:" line, returning None for "[DONE]".
pub fn parse_sse_data(line: &str) -> Option<&str> {
    let data = line.strip_prefix("data: ")?;
    if data == "[DONE]" {
        return None;
    }
    Some(data)
}

/// Extract a retryable error from an HTTP status code.
pub fn status_to_error(status: u16, body: &str) -> RociError {
    match status {
        401 | 403 => RociError::Authentication(body.to_string()),
        429 => RociError::RateLimited {
            retry_after_ms: extract_retry_after(body),
        },
        _ => RociError::api(status, body),
    }
}

fn extract_retry_after(body: &str) -> Option<u64> {
    // Try to parse retry-after from JSON error body
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("retry_after"))
                .and_then(|r| r.as_f64())
                .map(|s| (s * 1000.0) as u64)
        })
}
