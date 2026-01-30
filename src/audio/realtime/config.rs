//! Realtime session configuration.

use super::super::types::{AudioFormat, Voice};

/// Configuration for a realtime audio session.
#[derive(Debug, Clone)]
pub struct RealtimeConfiguration {
    pub model: String,
    pub voice: Option<Voice>,
    pub input_format: AudioFormat,
    pub output_format: AudioFormat,
    pub turn_detection: bool,
}

impl Default for RealtimeConfiguration {
    fn default() -> Self {
        Self {
            model: "gpt-4o-realtime-preview".to_string(),
            voice: None,
            input_format: AudioFormat::Pcm16,
            output_format: AudioFormat::Pcm16,
            turn_detection: true,
        }
    }
}
