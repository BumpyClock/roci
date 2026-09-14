use super::*;
use roci_core::auth::ProviderTokenMetadata;
use roci_core::provider::ToolDefinition;
use wiremock::{
    matchers::{header, method, path, query_param},
    Mock, MockServer, ResponseTemplate,
};

fn token() -> Token {
    Token {
        provider_metadata: Some(ProviderTokenMetadata::Gemini {
            project_id: "project-one".into(),
        }),
        access_token: "oauth-access".into(),
        refresh_token: Some("refresh".into()),
        id_token: None,
        expires_at: None,
        last_refresh: None,
        scopes: None,
        account_id: None,
    }
}

fn request() -> ProviderRequest {
    ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: GenerationSettings::default(),
        tools: Some(vec![ToolDefinition {
            name: "weather".into(),
            description: "Get weather".into(),
            parameters: json!({"type":"object","properties":{"city":{"type":"string"}}}),
        }]),
        response_format: None,
        api_key_override: None,
        headers: Default::default(),
        metadata: Default::default(),
        payload_callback: None,
        session_id: None,
        transport: None,
    }
}

#[tokio::test]
async fn native_generation_wraps_gemini_payload_and_maps_tools_usage_thoughts() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v1internal:generateContent")).and(header("authorization", "Bearer oauth-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"response":{
            "candidates":[{"content":{"parts":[{"text":"thinking", "thought":true,"thoughtSignature":"sig"},{"text":"sunny"},{"functionCall":{"id":"call-one","name":"weather","args":{"city":"Paris"}},"thoughtSignature":"call-sig"}]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":5,"totalTokenCount":17,"cachedContentTokenCount":2,"thoughtsTokenCount":3}
        }}))).expect(1).mount(&server).await;
    let provider = GeminiCliProvider::new(GoogleModel::Gemini25Flash, &token())
        .unwrap()
        .with_endpoint(server.uri());
    let response = provider.generate_text(&request()).await.unwrap();
    assert_eq!(response.text, "sunny");
    assert_eq!(response.tool_calls[0].name, "weather");
    assert_eq!(
        response.tool_calls[0].recipient.as_deref(),
        Some("call-sig")
    );
    assert_eq!(response.finish_reason, Some(FinishReason::ToolCalls));
    assert_eq!(response.usage.input_tokens, 12);
    assert_eq!(response.usage.reasoning_tokens, Some(3));
    assert!(
        matches!(&response.thinking[0], ContentPart::Thinking(thought) if thought.thinking == "thinking" && thought.signature == "sig")
    );
    let requests = server.received_requests().await.unwrap();
    let payload: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(payload["project"], "project-one");
    assert_eq!(payload["model"], "gemini-2.5-flash");
    assert_eq!(
        payload["request"]["contents"][0]["parts"][0]["text"],
        "hello"
    );
    assert_eq!(
        payload["request"]["tools"][0]["functionDeclarations"][0]["name"],
        "weather"
    );
    assert!(!requests[0].url.query().unwrap_or("").contains("key="));
    assert!(!requests[0].headers.contains_key("x-goog-api-key"));
}

#[tokio::test]
async fn native_stream_emits_reasoning_text_tools_and_final_usage() {
    let server = MockServer::start().await;
    let frames = [
        json!({"response":{"candidates":[{"content":{"parts":[{"thought":true,"text":"considering"},{"text":"café"}]}}]}}),
        json!({"response":{"candidates":[{"content":{"parts":[{"functionCall":{"name":"weather","args":{}},"thoughtSignature":"signature"}]},"finishReason":"STOP"}]}}),
        json!({"response":{"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":3,"totalTokenCount":5}}}),
    ];
    // Last event deliberately has no terminating newline.
    let body = frames
        .iter()
        .map(|frame| format!("data: {frame}"))
        .collect::<Vec<_>>()
        .join("\r\n\r\n");
    Mock::given(path("/v1internal:streamGenerateContent"))
        .and(query_param("alt", "sse"))
        .and(header("authorization", "Bearer oauth-access"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let provider = GeminiCliProvider::new(GoogleModel::Gemini25Flash, &token())
        .unwrap()
        .with_endpoint(server.uri());
    let events = provider
        .stream_text(&request())
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>(),
        [
            StreamEventType::Reasoning,
            StreamEventType::TextDelta,
            StreamEventType::ToolCallDelta,
            StreamEventType::Done
        ]
    );
    assert_eq!(events[1].text, "café");
    assert_eq!(
        events[2].tool_call.as_ref().unwrap().recipient.as_deref(),
        Some("signature")
    );
    assert_eq!(events[3].usage.as_ref().unwrap().total_tokens, 5);
    assert_eq!(events[3].finish_reason, Some(FinishReason::ToolCalls));
}

#[tokio::test]
async fn authentication_status_and_embedded_stream_error_remain_typed_and_redacted() {
    let server = MockServer::start().await;
    Mock::given(path("/v1internal:generateContent"))
        .respond_with(ResponseTemplate::new(401).set_body_string("oauth-access secret details"))
        .mount(&server)
        .await;
    Mock::given(path("/v1internal:streamGenerateContent"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                "data: {\"error\":{\"code\":401,\"message\":\"oauth-access\"}}\n\n",
            ),
        )
        .mount(&server)
        .await;
    let provider = GeminiCliProvider::new(GoogleModel::Gemini25Flash, &token())
        .unwrap()
        .with_endpoint(server.uri());
    let error = provider.generate_text(&request()).await.unwrap_err();
    assert!(matches!(error, RociError::Api { status: 401, .. }));
    assert!(!error.to_string().contains("oauth-access"));
    let events = provider
        .stream_text(&request())
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        Err(RociError::Api { status: 401, .. })
    ));
    assert!(!events[0]
        .as_ref()
        .unwrap_err()
        .to_string()
        .contains("oauth-access"));
}

#[test]
fn sse_decoder_preserves_split_utf8_and_multiline_data() {
    let payload = "data: {\"response\":\n".as_bytes();
    let tail = "data: {\"text\":\"café\"}}\n\n".as_bytes();
    let mut frames = SseFrames::default();
    let mut output = Vec::new();
    for byte in payload.iter().chain(tail) {
        output.extend(frames.push(&[*byte], false).unwrap());
    }
    assert_eq!(output, ["{\"response\":\n{\"text\":\"café\"}}"]);
    assert!(frames.push(&[0xff, b'\n'], false).is_err());
}

#[test]
fn missing_project_and_invalid_tool_calls_fail_explicitly() {
    let mut token = token();
    token.provider_metadata = None;
    assert!(GeminiCliProvider::new(GoogleModel::Gemini25Flash, &token).is_err());
    assert!(StreamState::default().decode(&json!({"response":{"candidates":[{"content":{"parts":[{"functionCall":{"args":{}}}]}}]}})).is_err());
    assert!(StreamState::default()
        .decode(&json!({"unknown":true}))
        .is_err());
}

#[tokio::test]
async fn empty_successful_http_stream_is_a_protocol_error() {
    let server = MockServer::start().await;
    Mock::given(path("/v1internal:streamGenerateContent"))
        .respond_with(ResponseTemplate::new(200).set_body_string(": heartbeat\n\n"))
        .mount(&server)
        .await;
    let provider = GeminiCliProvider::new(GoogleModel::Gemini25Flash, &token())
        .unwrap()
        .with_endpoint(server.uri());
    let events = provider
        .stream_text(&request())
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert_eq!(events.len(), 1);
    assert!(events[0].is_err());
}
