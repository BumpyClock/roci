use super::*;

fn settings() -> GenerationSettings {
    GenerationSettings::default()
}

#[test]
fn openai_responses_headers_include_session_affinity() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
    let session_id = "session-1";
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: Some(session_id.to_string()),
        transport: None,
    };

    let headers = provider.build_headers(&request).expect("headers");
    assert_eq!(
        headers.get("session_id").unwrap().to_str().unwrap(),
        session_id
    );
    assert_eq!(
        headers
            .get("x-client-request-id")
            .unwrap()
            .to_str()
            .unwrap(),
        session_id
    );
}

#[test]
fn openai_responses_headers_omit_session_affinity_when_absent() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let headers = provider.build_headers(&request).expect("headers");
    assert!(headers.get("session_id").is_none());
    assert!(headers.get("x-client-request-id").is_none());
}

#[test]
fn codex_headers_include_session_affinity() {
    let provider = OpenAiResponsesProvider::new(
        OpenAiModel::Gpt5Nano,
        "test-key".to_string(),
        Some("https://chatgpt.com/backend-api/codex".to_string()),
        Some("acct-123".to_string()),
    );
    let session_id = "codex-session-1";
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: Some(session_id.to_string()),
        transport: None,
    };

    let headers = provider.build_headers(&request).expect("headers");
    assert_eq!(
        headers.get("session_id").unwrap().to_str().unwrap(),
        session_id
    );
    assert_eq!(
        headers
            .get("x-client-request-id")
            .unwrap()
            .to_str()
            .unwrap(),
        session_id
    );
}

#[test]
fn codex_headers_omit_session_affinity_when_absent() {
    let provider = OpenAiResponsesProvider::new(
        OpenAiModel::Gpt5Nano,
        "test-key".to_string(),
        Some("https://chatgpt.com/backend-api/codex".to_string()),
        Some("acct-123".to_string()),
    );
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let headers = provider.build_headers(&request).expect("headers");
    assert!(headers.get("session_id").is_none());
    assert!(headers.get("x-client-request-id").is_none());
}

#[test]
fn headers_merge_request_overrides_and_api_key_override() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "base-key".to_string(), None, None);
    let mut request_headers = reqwest::header::HeaderMap::new();
    request_headers.insert(
        "x-request-header",
        reqwest::header::HeaderValue::from_static("value"),
    );
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: Some("override-key".to_string()),
        headers: request_headers,
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: Some(roci_core::provider::TRANSPORT_PROXY.to_string()),
    };

    let headers = provider
        .build_headers(&request)
        .expect("headers should build");
    assert_eq!(
        headers
            .get(reqwest::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer override-key")
    );
    assert_eq!(
        headers
            .get("x-request-header")
            .and_then(|value| value.to_str().ok()),
        Some("value")
    );
    assert_eq!(
        headers
            .get("x-roci-transport")
            .and_then(|value| value.to_str().ok()),
        Some(roci_core::provider::TRANSPORT_PROXY)
    );
}

#[test]
fn headers_use_request_api_key_override_when_default_key_missing() {
    let provider = OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, String::new(), None, None);
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: Some("override-key".to_string()),
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let headers = provider
        .build_headers(&request)
        .expect("headers should build");

    assert_eq!(
        headers
            .get(reqwest::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer override-key")
    );
}

#[test]
fn headers_error_when_no_default_or_request_api_key() {
    let provider = OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, String::new(), None, None);
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let err = provider.build_headers(&request).unwrap_err();

    assert!(matches!(err, RociError::MissingCredential { .. }));
}

#[test]
fn payload_callback_receives_request_payload() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
    let captured_model = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let captured_model_for_hook = captured_model.clone();
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: Some(std::sync::Arc::new(move |payload| {
            *captured_model_for_hook.lock().expect("capture lock") = payload
                .get("model")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
        })),
        session_id: None,
        transport: None,
    };
    let body = provider.build_request_body(&request, false);
    provider.emit_payload_callback(&request, &body);

    assert_eq!(
        captured_model.lock().expect("capture lock").as_deref(),
        Some("gpt-5-nano")
    );
}
