//! Audio capabilities: transcription, TTS, and realtime.

pub mod openai;
mod openai_helpers;
pub mod realtime;
pub mod transcription;
pub mod tts;
pub mod types;

pub use openai::{OpenAiTtsProvider, OpenAiWhisperTranscriptionProvider};
pub use transcription::AudioProvider;
pub use tts::SpeechProvider;
pub use types::*;
