//! Convenience functions for common generation patterns.
//!
//! NOTE: These functions use `create_provider()` from the root crate in the
//! original code. In roci-core they are placeholder stubs that will be
//! wired through `ProviderRegistry` by the meta-crate. For now they use
//! `ProviderRegistry` directly -- callers must supply a pre-built provider.

use crate::error::RociError;
use crate::provider::ModelProvider;
use crate::types::*;

/// Simple text generation: provider + prompt -> text.
pub async fn generate(
    provider: &dyn ModelProvider,
    prompt: impl Into<String>,
) -> Result<String, RociError> {
    let messages = vec![ModelMessage::user(prompt)];
    let result =
        super::text::generate_text(provider, messages, GenerationSettings::default(), &[]).await?;
    Ok(result.text)
}

/// Simple streaming generation: provider + prompt -> stream.
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

/// Analyze content: provider + system prompt + content -> text.
pub async fn analyze(
    provider: &dyn ModelProvider,
    system: impl Into<String>,
    content: impl Into<String>,
) -> Result<String, RociError> {
    let messages = vec![ModelMessage::system(system), ModelMessage::user(content)];
    let result =
        super::text::generate_text(provider, messages, GenerationSettings::default(), &[]).await?;
    Ok(result.text)
}
