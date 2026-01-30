//! Audio transcription trait.

use async_trait::async_trait;

use crate::error::RociError;
use super::types::TranscriptionResult;

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
