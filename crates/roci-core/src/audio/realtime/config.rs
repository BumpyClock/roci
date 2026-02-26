//! Realtime session configuration.

use std::time::Duration;

use super::super::types::{AudioFormat, Voice};

/// Configuration for a realtime audio session.
#[derive(Debug, Clone)]
pub struct RealtimeConfiguration {
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
    pub voice: Option<Voice>,
    pub input_format: AudioFormat,
    pub output_format: AudioFormat,
    pub turn_detection: bool,
    pub heartbeat_interval: Duration,
    pub reconnect_max_attempts: usize,
    pub reconnect_base_delay: Duration,
    pub reconnect_max_delay: Duration,
}

impl Default for RealtimeConfiguration {
    fn default() -> Self {
        Self {
            api_key: None,
            base_url: "wss://api.openai.com/v1/realtime".to_string(),
            model: "gpt-4o-realtime-preview".to_string(),
            voice: None,
            input_format: AudioFormat::Pcm16,
            output_format: AudioFormat::Pcm16,
            turn_detection: true,
            heartbeat_interval: Duration::from_secs(15),
            reconnect_max_attempts: 5,
            reconnect_base_delay: Duration::from_millis(250),
            reconnect_max_delay: Duration::from_secs(5),
        }
    }
}
