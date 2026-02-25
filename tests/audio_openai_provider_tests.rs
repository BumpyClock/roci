#![cfg(feature = "audio")]

use std::time::Duration;

use roci::audio::{
    AudioFormat, AudioProvider, OpenAiTtsProvider, OpenAiWhisperTranscriptionProvider,
    SpeechProvider, SpeechRequest, Voice,
};
use roci::error::RociError;
use roci::util::retry::RetryPolicy;
use serde_json::json;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn test_retry_policy(max_attempts: u32) -> RetryPolicy {
    RetryPolicy {
        max_attempts,
        initial_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(1),
        multiplier: 1.0,
    }
}

fn speech_request() -> SpeechRequest {
    SpeechRequest {
        text: "hello world".to_string(),
        voice: Voice {
            id: "alloy".to_string(),
            name: None,
            provider: "openai".to_string(),
        },
        format: AudioFormat::Mp3,
        speed: Some(1.2),
    }
}

#[tokio::test]
async fn whisper_transcription_happy_path() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .and(header("authorization", "Bearer test-key"))
        .and(body_string_contains("name=\"model\""))
        .and(body_string_contains("whisper-1"))
        .and(body_string_contains("name=\"language\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "text": "hello world",
            "language": "en",
            "duration": 2.4,
            "segments": [
                {"text": "hello", "start": 0.0, "end": 1.0},
                {"text": "world", "start": 1.0, "end": 2.4}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let provider =
        OpenAiWhisperTranscriptionProvider::new_with_base_url("test-key".to_string(), server.uri())
            .with_retry_policy(test_retry_policy(1));

    let result = provider
        .transcribe(b"RIFFfakewav", "audio/wav", Some("en"))
        .await
        .expect("transcription should succeed");

    assert_eq!(result.text, "hello world");
    assert_eq!(result.language.as_deref(), Some("en"));
    assert_eq!(result.duration_seconds, Some(2.4));
    let segments = result.segments.expect("segments");
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].text, "hello");
}

#[tokio::test]
async fn whisper_transcription_rejects_invalid_mime() {
    let provider = OpenAiWhisperTranscriptionProvider::new("test-key".to_string())
        .with_retry_policy(test_retry_policy(1));

    let err = provider
        .transcribe(b"audio", "text/plain", None)
        .await
        .expect_err("invalid mime should fail");

    assert!(
        matches!(err, RociError::InvalidArgument(message) if message.contains("Unsupported transcription MIME type"))
    );
}

#[tokio::test]
async fn whisper_transcription_handles_malformed_json() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_bytes(b"{not-json".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;

    let provider =
        OpenAiWhisperTranscriptionProvider::new_with_base_url("test-key".to_string(), server.uri())
            .with_retry_policy(test_retry_policy(1));

    let err = provider
        .transcribe(b"fake", "audio/mpeg", None)
        .await
        .expect_err("malformed json should fail");

    assert!(matches!(err, RociError::Serialization(_)));
}

#[tokio::test]
async fn whisper_transcription_retries_server_errors() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("oops"))
        .expect(3)
        .mount(&server)
        .await;

    let provider =
        OpenAiWhisperTranscriptionProvider::new_with_base_url("test-key".to_string(), server.uri())
            .with_retry_policy(test_retry_policy(3));

    let err = provider
        .transcribe(b"fake", "audio/mpeg", None)
        .await
        .expect_err("server error should bubble up after retries");

    assert!(matches!(err, RociError::Api { status: 500, .. }));
}

#[tokio::test]
async fn tts_happy_path_maps_voice_speed_and_format() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/audio/speech"))
        .and(header("authorization", "Bearer test-key"))
        .and(body_string_contains("\"voice\":\"alloy\""))
        .and(body_string_contains("\"response_format\":\"mp3\""))
        .and(body_string_contains("\"speed\":1.2"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/mpeg")
                .set_body_bytes(vec![1_u8, 2, 3, 4]),
        )
        .expect(1)
        .mount(&server)
        .await;

    let provider = OpenAiTtsProvider::new_with_base_url("test-key".to_string(), server.uri())
        .with_retry_policy(test_retry_policy(1));

    let audio = provider
        .generate_speech(&speech_request())
        .await
        .expect("tts should succeed");

    assert_eq!(audio, vec![1, 2, 3, 4]);
}

#[tokio::test]
async fn tts_rejects_invalid_speed() {
    let provider =
        OpenAiTtsProvider::new("test-key".to_string()).with_retry_policy(test_retry_policy(1));

    let mut request = speech_request();
    request.speed = Some(10.0);

    let err = provider
        .generate_speech(&request)
        .await
        .expect_err("invalid speed should fail");

    assert!(
        matches!(err, RociError::InvalidArgument(message) if message.contains("between 0.25 and 4.0"))
    );
}

#[tokio::test]
async fn tts_rejects_mismatched_content_type() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/plain")
                .set_body_string("not-audio"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let provider = OpenAiTtsProvider::new_with_base_url("test-key".to_string(), server.uri())
        .with_retry_policy(test_retry_policy(1));

    let err = provider
        .generate_speech(&speech_request())
        .await
        .expect_err("invalid mime should fail");

    assert!(
        matches!(err, RociError::InvalidState(message) if message.contains("Unexpected speech response MIME type"))
    );
}

#[tokio::test]
async fn tts_handles_json_error_payload() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({"error": {"message": "bad voice"}})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let provider = OpenAiTtsProvider::new_with_base_url("test-key".to_string(), server.uri())
        .with_retry_policy(test_retry_policy(1));

    let err = provider
        .generate_speech(&speech_request())
        .await
        .expect_err("json error payload should fail");

    assert!(
        matches!(err, RociError::Provider { provider, message } if provider == "openai" && message.contains("bad voice"))
    );
}

#[tokio::test]
async fn tts_timeout_maps_to_roci_timeout() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(80))
                .insert_header("content-type", "audio/mpeg")
                .set_body_bytes(vec![1_u8]),
        )
        .expect(1)
        .mount(&server)
        .await;

    let provider = OpenAiTtsProvider::new_with_base_url("test-key".to_string(), server.uri())
        .with_timeout(Duration::from_millis(10))
        .with_retry_policy(test_retry_policy(1));

    let err = provider
        .generate_speech(&speech_request())
        .await
        .expect_err("request should time out");

    assert!(matches!(err, RociError::Timeout(ms) if ms == 10));
}
