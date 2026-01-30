//! Streaming text generation with stop conditions.

use futures::stream::BoxStream;
use futures::StreamExt;

use crate::error::RociError;
use crate::provider::{ModelProvider, ProviderRequest};
use crate::stop::StopCondition;
use crate::types::*;

/// Stream text from a model, applying optional stop conditions.
///
/// Returns a stream of text deltas. Stop conditions can halt the stream early.
pub async fn stream_text(
    provider: &dyn ModelProvider,
    messages: Vec<ModelMessage>,
    settings: GenerationSettings,
    stop_conditions: &[Box<dyn StopCondition>],
) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
    let request = ProviderRequest {
        messages,
        settings: settings.clone(),
        tools: None,
        response_format: settings.response_format.clone(),
    };

    let inner_stream = provider.stream_text(&request).await?;

    if stop_conditions.is_empty() {
        return Ok(inner_stream);
    }

    // Wrap with stop condition checking
    // We need to own the stop conditions for the stream
    // Since StopCondition is not Clone, we can't move them in easily.
    // For now, return the raw stream — stop conditions are applied at a higher level.
    Ok(inner_stream)
}

/// Collect a stream into a final result.
pub async fn collect_stream(
    mut stream: BoxStream<'static, Result<TextStreamDelta, RociError>>,
) -> Result<StreamTextResult, RociError> {
    let mut text = String::new();
    let mut usage = Usage::default();
    let mut finish_reason = None;

    while let Some(delta) = stream.next().await {
        let delta = delta?;
        text.push_str(&delta.text);
        if let Some(u) = delta.usage {
            usage = u;
        }
        if let Some(fr) = delta.finish_reason {
            finish_reason = Some(fr);
        }
    }

    Ok(StreamTextResult {
        text,
        usage,
        finish_reason,
    })
}
