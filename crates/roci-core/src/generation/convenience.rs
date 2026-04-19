//! Convenience wrappers around the core generation entry points.
//!
//! These helpers do not resolve providers from configuration or a registry.
//! Callers must pass an already constructed provider; the helpers only turn a
//! single prompt into a user message and run with default settings.

use crate::error::RociError;
use crate::provider::ModelProvider;
use crate::types::*;

/// Generate text from a single user prompt with default settings and no tools.
pub async fn generate(
    provider: &dyn ModelProvider,
    prompt: impl Into<String>,
) -> Result<String, RociError> {
    let messages = vec![ModelMessage::user(prompt)];
    let result =
        super::text::generate_text(provider, messages, GenerationSettings::default(), &[]).await?;
    Ok(result.text)
}

/// Stream text from a single user prompt with default settings and no stop conditions.
pub async fn stream(
    provider: std::sync::Arc<dyn ModelProvider>,
    prompt: impl Into<String>,
) -> Result<futures::stream::BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
    let messages = vec![ModelMessage::user(prompt)];
    super::stream::stream_text(
        provider,
        messages,
        GenerationSettings::default(),
        Vec::new(),
    )
    .await
}
