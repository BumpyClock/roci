//! Convenience functions for common generation patterns.

use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider;
use crate::types::*;

/// Simple text generation: model + prompt → text.
pub async fn generate(
    model: &LanguageModel,
    prompt: impl Into<String>,
) -> Result<String, RociError> {
    let config = RociConfig::global();
    let provider = provider::create_provider(model, config)?;
    let messages = vec![ModelMessage::user(prompt)];
    let result =
        super::text::generate_text(provider.as_ref(), messages, GenerationSettings::default(), &[])
            .await?;
    Ok(result.text)
}

/// Simple streaming generation: model + prompt → stream.
pub async fn stream(
    model: &LanguageModel,
    prompt: impl Into<String>,
) -> Result<futures::stream::BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
    let config = RociConfig::global();
    let provider = provider::create_provider(model, config)?;
    let messages = vec![ModelMessage::user(prompt)];
    super::stream::stream_text(provider.as_ref(), messages, GenerationSettings::default(), &[]).await
}

/// Analyze content: model + system prompt + content → text.
pub async fn analyze(
    model: &LanguageModel,
    system: impl Into<String>,
    content: impl Into<String>,
) -> Result<String, RociError> {
    let config = RociConfig::global();
    let provider = provider::create_provider(model, config)?;
    let messages = vec![
        ModelMessage::system(system),
        ModelMessage::user(content),
    ];
    let result =
        super::text::generate_text(provider.as_ref(), messages, GenerationSettings::default(), &[])
            .await?;
    Ok(result.text)
}
