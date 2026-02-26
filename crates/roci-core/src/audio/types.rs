//! Audio-related types.

use serde::{Deserialize, Serialize};

/// Audio format.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Mp3,
    Opus,
    Aac,
    Flac,
    Wav,
    Pcm16,
}

/// Voice for text-to-speech.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Voice {
    pub id: String,
    pub name: Option<String>,
    pub provider: String,
}

/// Result of audio transcription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResult {
    pub text: String,
    pub language: Option<String>,
    pub duration_seconds: Option<f64>,
    pub segments: Option<Vec<TranscriptionSegment>>,
}

/// A segment within a transcription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionSegment {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// Request for speech synthesis.
#[derive(Debug, Clone)]
pub struct SpeechRequest {
    pub text: String,
    pub voice: Voice,
    pub format: AudioFormat,
    pub speed: Option<f64>,
}
