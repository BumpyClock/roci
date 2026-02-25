//! Tests for stream transforms.

use std::time::Duration;

use futures::stream::BoxStream;
use futures::StreamExt;
use roci::error::RociError;
use roci::stream_transform::{
    BufferTransform, FilterTransform, MapTransform, StreamTransform, ThrottleTransform,
};
use roci::types::{FinishReason, StreamEventType, TextStreamDelta};

fn delta(text: &str) -> TextStreamDelta {
    TextStreamDelta {
        text: text.to_string(),
        event_type: StreamEventType::TextDelta,
        tool_call: None,
        finish_reason: None,
        usage: None,
        reasoning: None,
        reasoning_signature: None,
        reasoning_type: None,
    }
}

fn done(reason: FinishReason) -> TextStreamDelta {
    TextStreamDelta {
        text: String::new(),
        event_type: StreamEventType::Done,
        tool_call: None,
        finish_reason: Some(reason),
        usage: None,
        reasoning: None,
        reasoning_signature: None,
        reasoning_type: None,
    }
}

fn boxed_stream(
    items: Vec<Result<TextStreamDelta, RociError>>,
) -> BoxStream<'static, Result<TextStreamDelta, RociError>> {
    futures::stream::iter(items).boxed()
}

#[tokio::test]
async fn filter_transform_keeps_matching_deltas_in_order() {
    let transform = FilterTransform::new(|item: &TextStreamDelta| item.text != "drop");
    let stream = boxed_stream(vec![
        Ok(delta("keep-1")),
        Ok(delta("drop")),
        Ok(delta("keep-2")),
        Ok(done(FinishReason::Stop)),
    ]);

    let items = transform.transform(stream).collect::<Vec<_>>().await;

    assert_eq!(items.len(), 3);
    assert_eq!(items[0].as_ref().unwrap().text, "keep-1");
    assert_eq!(items[1].as_ref().unwrap().text, "keep-2");
    assert_eq!(
        items[2].as_ref().unwrap().finish_reason,
        Some(FinishReason::Stop)
    );
}

#[tokio::test]
async fn filter_transform_propagates_error_and_stops() {
    let transform = FilterTransform::new(|_: &TextStreamDelta| true);
    let stream = boxed_stream(vec![
        Ok(delta("keep")),
        Err(RociError::Stream("filter boom".to_string())),
        Ok(delta("late")),
    ]);

    let items = transform.transform(stream).collect::<Vec<_>>().await;

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].as_ref().unwrap().text, "keep");
    match &items[1] {
        Err(RociError::Stream(message)) => assert_eq!(message, "filter boom"),
        other => panic!("expected stream error, got {other:?}"),
    }
}

#[tokio::test]
async fn map_transform_rewrites_non_empty_text_and_keeps_metadata() {
    let transform = MapTransform::new(|text| text.to_uppercase());
    let stream = boxed_stream(vec![Ok(delta("hello")), Ok(done(FinishReason::Stop))]);

    let items = transform.transform(stream).collect::<Vec<_>>().await;

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].as_ref().unwrap().text, "HELLO");
    assert_eq!(items[1].as_ref().unwrap().text, "");
    assert_eq!(
        items[1].as_ref().unwrap().finish_reason,
        Some(FinishReason::Stop)
    );
}

#[tokio::test]
async fn map_transform_propagates_error_and_stops() {
    let transform = MapTransform::new(|text| format!("mapped-{text}"));
    let stream = boxed_stream(vec![
        Ok(delta("a")),
        Err(RociError::Stream("map boom".to_string())),
        Ok(delta("late")),
    ]);

    let items = transform.transform(stream).collect::<Vec<_>>().await;

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].as_ref().unwrap().text, "mapped-a");
    match &items[1] {
        Err(RociError::Stream(message)) => assert_eq!(message, "map boom"),
        other => panic!("expected stream error, got {other:?}"),
    }
}

#[tokio::test]
async fn buffer_transform_emits_on_threshold_and_preserves_order() {
    let transform = BufferTransform::new(5);
    let stream = boxed_stream(vec![
        Ok(delta("ab")),
        Ok(delta("cd")),
        Ok(delta("ef")),
        Ok(done(FinishReason::Stop)),
    ]);

    let items = transform.transform(stream).collect::<Vec<_>>().await;

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].as_ref().unwrap().text, "abcdef");
    assert_eq!(items[1].as_ref().unwrap().text, "");
    assert_eq!(
        items[1].as_ref().unwrap().finish_reason,
        Some(FinishReason::Stop)
    );
}

#[tokio::test]
async fn buffer_transform_flushes_remaining_text_on_stream_end() {
    let transform = BufferTransform::new(10);
    let stream = boxed_stream(vec![Ok(delta("hello")), Ok(delta(" world"))]);

    let items = transform.transform(stream).collect::<Vec<_>>().await;

    assert_eq!(items.len(), 1);
    let flushed = items[0].as_ref().unwrap();
    assert_eq!(flushed.text, "hello world");
    assert_eq!(flushed.event_type, StreamEventType::TextDelta);
    assert!(flushed.finish_reason.is_none());
}

#[tokio::test]
async fn buffer_transform_propagates_error_and_continues_processing() {
    let transform = BufferTransform::new(3);
    let stream = boxed_stream(vec![
        Ok(delta("ab")),
        Err(RociError::Stream("buffer boom".to_string())),
        Ok(delta("cd")),
        Ok(done(FinishReason::Stop)),
    ]);

    let items = transform.transform(stream).collect::<Vec<_>>().await;

    assert_eq!(items.len(), 3);
    match &items[0] {
        Err(RociError::Stream(message)) => assert_eq!(message, "buffer boom"),
        other => panic!("expected stream error, got {other:?}"),
    }
    assert_eq!(items[1].as_ref().unwrap().text, "abcd");
    assert_eq!(
        items[2].as_ref().unwrap().finish_reason,
        Some(FinishReason::Stop)
    );
}

#[tokio::test]
async fn throttle_transform_with_zero_interval_preserves_order() {
    let transform = ThrottleTransform::new(Duration::ZERO);
    let stream = boxed_stream(vec![
        Ok(delta("a")),
        Ok(delta("b")),
        Ok(done(FinishReason::Stop)),
    ]);

    let items = transform.transform(stream).collect::<Vec<_>>().await;

    assert_eq!(items.len(), 3);
    assert_eq!(items[0].as_ref().unwrap().text, "a");
    assert_eq!(items[1].as_ref().unwrap().text, "b");
    assert_eq!(
        items[2].as_ref().unwrap().finish_reason,
        Some(FinishReason::Stop)
    );
}

#[tokio::test]
async fn throttle_transform_batches_and_flushes_remaining_text() {
    let transform = ThrottleTransform::new(Duration::from_secs(60));
    let stream = boxed_stream(vec![
        Ok(delta("a")),
        Ok(delta("b")),
        Ok(delta("c")),
        Ok(done(FinishReason::Stop)),
    ]);

    let items = transform.transform(stream).collect::<Vec<_>>().await;

    assert_eq!(items.len(), 1);
    let flushed = items[0].as_ref().unwrap();
    assert_eq!(flushed.text, "abc");
    assert_eq!(flushed.finish_reason, Some(FinishReason::Stop));
}

#[tokio::test]
async fn throttle_transform_propagates_error_and_flushes_buffer() {
    let transform = ThrottleTransform::new(Duration::from_secs(60));
    let stream = boxed_stream(vec![
        Ok(delta("ab")),
        Err(RociError::Stream("throttle boom".to_string())),
    ]);

    let items = transform.transform(stream).collect::<Vec<_>>().await;

    assert_eq!(items.len(), 2);
    match &items[0] {
        Err(RociError::Stream(message)) => assert_eq!(message, "throttle boom"),
        other => panic!("expected stream error, got {other:?}"),
    }
    assert_eq!(items[1].as_ref().unwrap().text, "ab");
}
