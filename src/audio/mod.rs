//! Audio capabilities: transcription, TTS, and realtime.

pub mod realtime;
pub mod transcription;
pub mod tts;
pub mod types;

pub use transcription::AudioProvider;
pub use tts::SpeechProvider;
pub use types::*;
