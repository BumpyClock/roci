//! OpenAI audio providers (Whisper transcription + TTS).

use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use uuid::Uuid;

use super::openai_helpers::{
    build_transcription_multipart, content_type_matches_expected_audio,
    is_supported_transcription_mime, normalize_mime_type, transcription_extension_for_mime,
    trim_trailing_slash, tts_format_name,
};
use super::transcription::AudioProvider;
use super::tts::SpeechProvider;
use super::types::{AudioFormat, SpeechRequest, TranscriptionResult, TranscriptionSegment};
use crate::error::RociError;
use crate::provider::http::{bearer_headers, shared_client, status_to_error};
use crate::util::retry::RetryPolicy;
use crate::util::timeout::with_timeout;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_WHISPER_MODEL: &str = "whisper-1";
const DEFAULT_TTS_MODEL: &str = "tts-1";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// OpenAI Whisper transcription provider (`/audio/transcriptions`).
#[derive(Debug, Clone)]
pub struct OpenAiWhisperTranscriptionProvider {
    api_key: String,
    base_url: String,
    model: String,
    timeout: Duration,
    retry_policy: RetryPolicy,
}

impl OpenAiWhisperTranscriptionProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_WHISPER_MODEL.to_string(),
            timeout: DEFAULT_TIMEOUT,
            retry_policy: RetryPolicy::default(),
        }
    }

    pub fn new_with_base_url(api_key: String, base_url: impl Into<String>) -> Self {
        Self {
            api_key,
            base_url: base_url.into(),
            model: DEFAULT_WHISPER_MODEL.to_string(),
            timeout: DEFAULT_TIMEOUT,
            retry_policy: RetryPolicy::default(),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }

    fn validate_inputs(
        &self,
        audio: &[u8],
        mime_type: &str,
        language: Option<&str>,
    ) -> Result<String, RociError> {
        if self.api_key.trim().is_empty() {
            return Err(RociError::Authentication(
                "Missing OpenAI API key for audio transcription".to_string(),
            ));
        }
        if self.model.trim().is_empty() {
            return Err(RociError::InvalidArgument(
                "Transcription model cannot be empty".to_string(),
            ));
        }
        if audio.is_empty() {
            return Err(RociError::InvalidArgument(
                "Audio payload cannot be empty".to_string(),
            ));
        }

        let normalized_mime = normalize_mime_type(mime_type)
            .ok_or_else(|| RociError::InvalidArgument("MIME type cannot be empty".to_string()))?;
        if !is_supported_transcription_mime(normalized_mime) {
            return Err(RociError::InvalidArgument(format!(
                "Unsupported transcription MIME type: {normalized_mime}"
            )));
        }

        if let Some(lang) = language {
            if lang.trim().is_empty() {
                return Err(RociError::InvalidArgument(
                    "Language hint cannot be empty".to_string(),
                ));
            }
        }

        Ok(normalized_mime.to_string())
    }

    async fn transcribe_once(
        &self,
        audio: &[u8],
        mime_type: &str,
        language: Option<&str>,
    ) -> Result<TranscriptionResult, RociError> {
        let extension = transcription_extension_for_mime(mime_type).ok_or_else(|| {
            RociError::InvalidArgument(format!("Unsupported transcription MIME type: {mime_type}"))
        })?;

        let boundary = format!("roci-{}", Uuid::new_v4().simple());
        let multipart_body = build_transcription_multipart(
            &boundary,
            &self.model,
            audio,
            mime_type,
            extension,
            language,
        );

        let mut headers = bearer_headers(&self.api_key);
        headers.insert(
            CONTENT_TYPE,
            reqwest::header::HeaderValue::from_str(&format!(
                "multipart/form-data; boundary={boundary}"
            ))
            .map_err(|e| {
                RociError::InvalidArgument(format!("Failed to build multipart content-type: {e}"))
            })?,
        );

        let url = format!(
            "{}/audio/transcriptions",
            trim_trailing_slash(&self.base_url)
        );

        with_timeout(self.timeout, async {
            let response = shared_client()
                .post(url)
                .headers(headers)
                .body(multipart_body)
                .send()
                .await?;

            parse_transcription_response(response).await
        })
        .await
    }
}

#[async_trait]
impl AudioProvider for OpenAiWhisperTranscriptionProvider {
    async fn transcribe(
        &self,
        audio: &[u8],
        mime_type: &str,
        language: Option<&str>,
    ) -> Result<TranscriptionResult, RociError> {
        let normalized_mime = self.validate_inputs(audio, mime_type, language)?;

        self.retry_policy
            .execute(|| self.transcribe_once(audio, &normalized_mime, language))
            .await
    }
}

/// OpenAI TTS provider (`/audio/speech`).
#[derive(Debug, Clone)]
pub struct OpenAiTtsProvider {
    api_key: String,
    base_url: String,
    model: String,
    timeout: Duration,
    retry_policy: RetryPolicy,
}

impl OpenAiTtsProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_TTS_MODEL.to_string(),
            timeout: DEFAULT_TIMEOUT,
            retry_policy: RetryPolicy::default(),
        }
    }

    pub fn new_with_base_url(api_key: String, base_url: impl Into<String>) -> Self {
        Self {
            api_key,
            base_url: base_url.into(),
            model: DEFAULT_TTS_MODEL.to_string(),
            timeout: DEFAULT_TIMEOUT,
            retry_policy: RetryPolicy::default(),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }

    fn validate_request(&self, request: &SpeechRequest) -> Result<(), RociError> {
        if self.api_key.trim().is_empty() {
            return Err(RociError::Authentication(
                "Missing OpenAI API key for speech generation".to_string(),
            ));
        }
        if request.text.trim().is_empty() {
            return Err(RociError::InvalidArgument(
                "Speech text cannot be empty".to_string(),
            ));
        }
        if request.voice.id.trim().is_empty() {
            return Err(RociError::InvalidArgument(
                "Voice id cannot be empty".to_string(),
            ));
        }
        if let Some(speed) = request.speed {
            if !speed.is_finite() || !(0.25..=4.0).contains(&speed) {
                return Err(RociError::InvalidArgument(
                    "Speech speed must be between 0.25 and 4.0".to_string(),
                ));
            }
        }
        Ok(())
    }

    async fn generate_speech_once(&self, request: &SpeechRequest) -> Result<Vec<u8>, RociError> {
        let mut payload = serde_json::json!({
            "model": self.model.clone(),
            "input": request.text.clone(),
            "voice": request.voice.id.clone(),
            "response_format": tts_format_name(request.format),
        });
        if let Some(speed) = request.speed {
            payload["speed"] = serde_json::json!(speed);
        }

        let url = format!("{}/audio/speech", trim_trailing_slash(&self.base_url));
        let headers = bearer_headers(&self.api_key);

        with_timeout(self.timeout, async {
            let response = shared_client()
                .post(url)
                .headers(headers)
                .json(&payload)
                .send()
                .await?;

            parse_tts_response(response, request.format).await
        })
        .await
    }
}

#[async_trait]
impl SpeechProvider for OpenAiTtsProvider {
    async fn generate_speech(&self, request: &SpeechRequest) -> Result<Vec<u8>, RociError> {
        self.validate_request(request)?;
        self.retry_policy
            .execute(|| self.generate_speech_once(request))
            .await
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiTranscriptionResponse {
    text: String,
    language: Option<String>,
    duration: Option<f64>,
    segments: Option<Vec<OpenAiTranscriptionSegment>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiTranscriptionSegment {
    text: String,
    start: f64,
    end: f64,
}

async fn parse_transcription_response(
    response: reqwest::Response,
) -> Result<TranscriptionResult, RociError> {
    let status = response.status().as_u16();
    if status != 200 {
        let body = response.text().await.unwrap_or_default();
        return Err(status_to_error(status, &body));
    }

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if !content_type.starts_with("application/json") {
        return Err(RociError::InvalidState(format!(
            "Expected JSON transcription response, got '{content_type}'"
        )));
    }

    let body = response.text().await?;
    let parsed: OpenAiTranscriptionResponse = serde_json::from_str(&body)?;
    if parsed.text.trim().is_empty() {
        return Err(RociError::InvalidState(
            "Transcription response missing text".to_string(),
        ));
    }

    Ok(TranscriptionResult {
        text: parsed.text,
        language: parsed.language,
        duration_seconds: parsed.duration,
        segments: parsed.segments.map(|segments| {
            segments
                .into_iter()
                .map(|segment| TranscriptionSegment {
                    text: segment.text,
                    start: segment.start,
                    end: segment.end,
                })
                .collect()
        }),
    })
}

async fn parse_tts_response(
    response: reqwest::Response,
    format: AudioFormat,
) -> Result<Vec<u8>, RociError> {
    let status = response.status().as_u16();
    if status != 200 {
        let body = response.text().await.unwrap_or_default();
        return Err(status_to_error(status, &body));
    }

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if content_type.starts_with("application/json") {
        let body = response.text().await.unwrap_or_default();
        if let Some(message) = extract_openai_error_message(&body) {
            return Err(RociError::Provider {
                provider: "openai".to_string(),
                message,
            });
        }
        return Err(RociError::InvalidState(
            "Expected audio payload, got JSON response".to_string(),
        ));
    }

    if !content_type_matches_expected_audio(&content_type, format) {
        return Err(RociError::InvalidState(format!(
            "Unexpected speech response MIME type '{content_type}' for format {:?}",
            format
        )));
    }

    let bytes = response.bytes().await?;
    if bytes.is_empty() {
        return Err(RociError::InvalidState(
            "Speech response contained empty audio payload".to_string(),
        ));
    }

    Ok(bytes.to_vec())
}

fn extract_openai_error_message(body: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    parsed
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(|message| message.as_str())
        .map(ToString::to_string)
}
