//! Audio transcription trait.

use async_trait::async_trait;

use super::types::TranscriptionResult;
use crate::error::RociError;

/// Trait for audio transcription providers.
#[async_trait]
pub trait AudioProvider: Send + Sync {
    /// Transcribe audio data.
    async fn transcribe(
        &self,
        audio: &[u8],
        mime_type: &str,
        language: Option<&str>,
    ) -> Result<TranscriptionResult, RociError>;
}
