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
pub struct FilterTransform {
    predicate: std::sync::Arc<dyn Fn(&TextStreamDelta) -> bool + Send + Sync>,
}

impl FilterTransform {
    pub fn new<F>(predicate: F) -> Self
    where
        F: Fn(&TextStreamDelta) -> bool + Send + Sync + 'static,
    {
        Self {
            predicate: std::sync::Arc::new(predicate),
        }
    }
}

impl StreamTransform for FilterTransform {
    fn transform(
        &self,
        stream: BoxStream<'static, Result<TextStreamDelta, RociError>>,
    ) -> BoxStream<'static, Result<TextStreamDelta, RociError>> {
        let predicate = self.predicate.clone();
        let transformed = async_stream::stream! {
            let mut inner = std::pin::pin!(stream);
            while let Some(item) = inner.next().await {
                match item {
                    Ok(delta) => {
                        if (predicate)(&delta) {
                            yield Ok(delta);
                        }
                    }
                    Err(e) => {
                        yield Err(e);
                        break;
                    }
                }
            }
        };
        Box::pin(transformed)
    }
}

/// Map/transform each delta's text.
pub struct MapTransform {
    mapper: std::sync::Arc<dyn Fn(String) -> String + Send + Sync>,
}

impl MapTransform {
    pub fn new<F>(mapper: F) -> Self
    where
        F: Fn(String) -> String + Send + Sync + 'static,
    {
        Self {
            mapper: std::sync::Arc::new(mapper),
        }
    }
}

impl StreamTransform for MapTransform {
    fn transform(
        &self,
        stream: BoxStream<'static, Result<TextStreamDelta, RociError>>,
    ) -> BoxStream<'static, Result<TextStreamDelta, RociError>> {
        let mapper = self.mapper.clone();
        let transformed = async_stream::stream! {
            let mut inner = std::pin::pin!(stream);
            while let Some(item) = inner.next().await {
                match item {
                    Ok(mut delta) => {
                        if !delta.text.is_empty() {
                            delta.text = (mapper)(delta.text);
                        }
                        yield Ok(delta);
                    }
                    Err(e) => {
                        yield Err(e);
                        break;
                    }
                }
            }
        };
        Box::pin(transformed)
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
                    tool_call: None,
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
