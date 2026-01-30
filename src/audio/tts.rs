//! Text-to-speech trait.

use async_trait::async_trait;

use crate::error::RociError;
use super::types::SpeechRequest;

/// Trait for text-to-speech providers.
#[async_trait]
pub trait SpeechProvider: Send + Sync {
    /// Generate speech audio from text.
    async fn generate_speech(&self, request: &SpeechRequest) -> Result<Vec<u8>, RociError>;
}
