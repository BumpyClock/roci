//! Header building for OpenAI Responses requests.

use roci_core::error::RociError;
use roci_core::provider::http::bearer_headers;
use roci_core::provider::{ModelProvider, ProviderRequest};
use roci_core::util::debug::roci_debug_enabled;

use super::OpenAiResponsesProvider;

impl OpenAiResponsesProvider {
    fn resolved_api_key<'a>(&'a self, request: &'a ProviderRequest) -> Result<&'a str, RociError> {
        let default_key = (!self.api_key.is_empty()).then_some(self.api_key.as_str());
        request
            .api_key_override
            .as_deref()
            .or(default_key)
            .ok_or_else(|| RociError::MissingCredential {
                provider: self.provider_name().to_string(),
            })
    }

    fn add_session_affinity_headers(headers: &mut reqwest::header::HeaderMap, session_id: &str) {
        if let Ok(value) = reqwest::header::HeaderValue::from_str(session_id) {
            headers.insert("session_id", value.clone());
            headers.insert("x-client-request-id", value);
        }
    }

    pub(super) fn build_headers(
        &self,
        request: &ProviderRequest,
    ) -> Result<reqwest::header::HeaderMap, RociError> {
        let resolved_api_key = self.resolved_api_key(request)?;
        let mut headers = bearer_headers(resolved_api_key);
        if self.is_codex {
            let account_id = match (&self.account_id, extract_codex_account_id(resolved_api_key)) {
                (Some(id), _) => Some(id.clone()),
                (None, Ok(id)) => Some(id),
                (None, Err(err)) => {
                    return Err(RociError::Authentication(format!(
                        "Missing Codex account id ({err})"
                    )))
                }
            };
            if let Some(account_id) = account_id {
                if let Ok(value) = reqwest::header::HeaderValue::from_str(&account_id) {
                    headers.insert("chatgpt-account-id", value);
                }
            }
            headers.insert(
                "OpenAI-Beta",
                reqwest::header::HeaderValue::from_static("responses=experimental"),
            );
            headers.insert(
                "originator",
                reqwest::header::HeaderValue::from_static("pi"),
            );
            headers.insert(
                reqwest::header::ACCEPT,
                reqwest::header::HeaderValue::from_static("text/event-stream"),
            );
            let user_agent = format!("roci ({} {})", std::env::consts::OS, std::env::consts::ARCH);
            if let Ok(value) = reqwest::header::HeaderValue::from_str(&user_agent) {
                headers.insert(reqwest::header::USER_AGENT, value);
            }
        } else if let Some(account_id) = &self.account_id {
            if let Ok(value) = reqwest::header::HeaderValue::from_str(account_id) {
                headers.insert("ChatGPT-Account-ID", value);
            }
        }
        if let Some(ref session_id) = request.session_id {
            Self::add_session_affinity_headers(&mut headers, session_id);
        }
        if let Some(ref transport) = request.transport {
            if let Ok(value) = reqwest::header::HeaderValue::from_str(transport) {
                headers.insert("x-roci-transport", value);
            }
        }
        for (name, value) in request.headers.iter() {
            headers.insert(name, value.clone());
        }
        if roci_debug_enabled() {
            tracing::debug!(
                model = self.model.as_str(),
                base_url = %self.base_url,
                account_id_present = self.account_id.is_some(),
                api_key_overridden = request.api_key_override.is_some(),
                request_header_overrides = request.headers.len(),
                codex_headers = self.is_codex,
                "OpenAI Responses headers prepared"
            );
        }
        Ok(headers)
    }
}

fn extract_codex_account_id(token: &str) -> Result<String, RociError> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    let mut parts = token.split('.');
    let _header = parts.next().ok_or_else(|| {
        RociError::Authentication("Invalid Codex token (missing JWT header)".into())
    })?;
    let payload = parts.next().ok_or_else(|| {
        RociError::Authentication("Invalid Codex token (missing JWT payload)".into())
    })?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| RociError::Authentication("Invalid Codex token payload encoding".into()))?;
    let value: serde_json::Value = serde_json::from_slice(&decoded)
        .map_err(|_| RociError::Authentication("Invalid Codex token payload JSON".into()))?;
    let account_id = value
        .get("https://api.openai.com/auth")
        .and_then(|v| v.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| RociError::Authentication("Missing Codex account id claim".into()))?;
    Ok(account_id.to_string())
}
