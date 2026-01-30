//! Tests for stream transforms.

use futures::StreamExt;
use roci::stream_transform::{FilterTransform, MapTransform, StreamTransform};
use roci::types::{FinishReason, StreamEventType, TextStreamDelta};

#[tokio::test]
async fn filter_transform_keeps_matching_deltas() {
    let stream = async_stream::stream! {
        yield Ok(TextStreamDelta {
            text: "keep".to_string(),
            event_type: StreamEventType::TextDelta,
            tool_call: None,
            finish_reason: None,
            usage: None,
            reasoning: None,
            reasoning_signature: None,
            reasoning_type: None,
        });
        yield Ok(TextStreamDelta {
            text: "drop".to_string(),
            event_type: StreamEventType::TextDelta,
            tool_call: None,
            finish_reason: None,
            usage: None,
            reasoning: None,
            reasoning_signature: None,
            reasoning_type: None,
        });
        yield Ok(TextStreamDelta {
            text: String::new(),
            event_type: StreamEventType::Done,
            tool_call: None,
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            reasoning: None,
            reasoning_signature: None,
            reasoning_type: None,
        });
    };

    let transform = FilterTransform::new(|delta: &TextStreamDelta| delta.text != "drop");
    let mut filtered = transform.transform(Box::pin(stream));

    let mut texts = Vec::new();
    while let Some(item) = filtered.next().await {
        let delta = item.unwrap();
        if !delta.text.is_empty() {
            texts.push(delta.text);
        }
    }

    assert_eq!(texts, vec!["keep".to_string()]);
}

#[tokio::test]
async fn map_transform_rewrites_text() {
    let stream = async_stream::stream! {
        yield Ok(TextStreamDelta {
            text: "hello".to_string(),
            event_type: StreamEventType::TextDelta,
            tool_call: None,
            finish_reason: None,
            usage: None,
            reasoning: None,
            reasoning_signature: None,
            reasoning_type: None,
        });
        yield Ok(TextStreamDelta {
            text: String::new(),
            event_type: StreamEventType::Done,
            tool_call: None,
            finish_reason: Some(FinishReason::Stop),
            usage: None,
            reasoning: None,
            reasoning_signature: None,
            reasoning_type: None,
        });
    };

    let transform = MapTransform::new(|text| text.to_uppercase());
    let mut mapped = transform.transform(Box::pin(stream));
    let mut result = String::new();
    while let Some(item) = mapped.next().await {
        let delta = item.unwrap();
        result.push_str(&delta.text);
    }

    assert_eq!(result, "HELLO");
}
