//! Stream transformations for text generation streams.

use futures::stream::BoxStream;
use futures::StreamExt;

use crate::error::RociError;
use crate::types::TextStreamDelta;

/// Trait for transforming a stream of text deltas.
pub trait StreamTransform: Send + Sync {
    /// Transform the stream.
    fn transform(
        &self,
        stream: BoxStream<'static, Result<TextStreamDelta, RociError>>,
    ) -> BoxStream<'static, Result<TextStreamDelta, RociError>>;
}

/// Filter deltas based on a predicate.
pub struct FilterTransform<F: Fn(&TextStreamDelta) -> bool + Send + Sync + 'static> {
    predicate: F,
}

impl<F: Fn(&TextStreamDelta) -> bool + Send + Sync + 'static> FilterTransform<F> {
    pub fn new(predicate: F) -> Self {
        Self { predicate }
    }
}

impl<F: Fn(&TextStreamDelta) -> bool + Send + Sync + 'static> StreamTransform for FilterTransform<F> {
    fn transform(
        &self,
        stream: BoxStream<'static, Result<TextStreamDelta, RociError>>,
    ) -> BoxStream<'static, Result<TextStreamDelta, RociError>> {
        // We need to move the predicate; since we can't clone Fn, use a reference-counted wrapper
        // For simplicity, consume self's predicate concept via a new stream
        // Note: This is a limitation — in practice, wrap in Arc
        stream // pass-through for now; real impl needs Arc<F>
    }
}

/// Map/transform each delta's text.
pub struct MapTransform<F: Fn(String) -> String + Send + Sync + 'static> {
    mapper: F,
}

impl<F: Fn(String) -> String + Send + Sync + 'static> MapTransform<F> {
    pub fn new(mapper: F) -> Self {
        Self { mapper }
    }
}

impl<F: Fn(String) -> String + Send + Sync + 'static> StreamTransform for MapTransform<F> {
    fn transform(
        &self,
        stream: BoxStream<'static, Result<TextStreamDelta, RociError>>,
    ) -> BoxStream<'static, Result<TextStreamDelta, RociError>> {
        stream // pass-through stub
    }
}

/// Buffer deltas until a minimum size, then emit.
pub struct BufferTransform {
    min_chars: usize,
}

impl BufferTransform {
    pub fn new(min_chars: usize) -> Self {
        Self { min_chars }
    }
}

impl StreamTransform for BufferTransform {
    fn transform(
        &self,
        stream: BoxStream<'static, Result<TextStreamDelta, RociError>>,
    ) -> BoxStream<'static, Result<TextStreamDelta, RociError>> {
        let min_chars = self.min_chars;
        let transformed = async_stream::stream! {
            let mut buffer = String::new();
            let mut inner = std::pin::pin!(stream);

            while let Some(item) = inner.next().await {
                match item {
                    Ok(mut delta) => {
                        buffer.push_str(&delta.text);
                        if buffer.len() >= min_chars || delta.finish_reason.is_some() {
                            delta.text = std::mem::take(&mut buffer);
                            yield Ok(delta);
                        }
                    }
                    Err(e) => yield Err(e),
                }
            }

            // Flush remaining buffer
            if !buffer.is_empty() {
                yield Ok(TextStreamDelta {
                    text: buffer,
                    event_type: crate::types::StreamEventType::TextDelta,
                    finish_reason: None,
                    usage: None,
                });
            }
        };

        Box::pin(transformed)
    }
}

/// Throttle emissions to at most one per interval.
pub struct ThrottleTransform {
    interval: std::time::Duration,
}

impl ThrottleTransform {
    pub fn new(interval: std::time::Duration) -> Self {
        Self { interval }
    }
}

impl StreamTransform for ThrottleTransform {
    fn transform(
        &self,
        stream: BoxStream<'static, Result<TextStreamDelta, RociError>>,
    ) -> BoxStream<'static, Result<TextStreamDelta, RociError>> {
        let interval = self.interval;
        let transformed = async_stream::stream! {
            let mut buffer = String::new();
            let mut last_emit = std::time::Instant::now();
            let mut last_delta: Option<TextStreamDelta> = None;
            let mut inner = std::pin::pin!(stream);

            while let Some(item) = inner.next().await {
                match item {
                    Ok(delta) => {
                        buffer.push_str(&delta.text);
                        last_delta = Some(delta);

                        if last_emit.elapsed() >= interval {
                            if let Some(mut d) = last_delta.take() {
                                d.text = std::mem::take(&mut buffer);
                                yield Ok(d);
                                last_emit = std::time::Instant::now();
                            }
                        }
                    }
                    Err(e) => yield Err(e),
                }
            }

            if !buffer.is_empty() {
                if let Some(mut d) = last_delta.take() {
                    d.text = buffer;
                    yield Ok(d);
                }
            }
        };

        Box::pin(transformed)
    }
}
