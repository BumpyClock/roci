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
    provider: std::sync::Arc<dyn ModelProvider>,
    messages: Vec<ModelMessage>,
    settings: GenerationSettings,
    stop_conditions: Vec<Box<dyn StopCondition>>,
) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
    let stream = async_stream::stream! {
        let mut accumulated_text = String::new();
        for cond in &stop_conditions {
            cond.reset().await;
        }

        let request = ProviderRequest {
            messages,
            settings: settings.clone(),
            tools: None,
            response_format: settings.response_format.clone(),
            api_key_override: None,
            headers: reqwest::header::HeaderMap::new(),
            metadata: std::collections::HashMap::new(),
            payload_callback: None,
            session_id: None,
            transport: None,
        };
        let mut inner = match provider.stream_text(&request).await {
            Ok(stream) => stream,
            Err(e) => {
                yield Err(e);
                return;
            }
        };
        while let Some(item) = inner.next().await {
            match item {
                Ok(delta) => {
                    let event_type = delta.event_type;
                    let delta_text = delta.text.clone();
                    if !delta_text.is_empty() {
                        accumulated_text.push_str(&delta_text);
                    }
                    yield Ok(delta);
                    if matches!(event_type, StreamEventType::TextDelta) {
                        let mut stop_triggered = false;
                        for cond in &stop_conditions {
                            if cond.should_stop(&accumulated_text, Some(&delta_text)).await {
                                stop_triggered = true;
                                break;
                            }
                        }
                        if stop_triggered {
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
                            break;
                        }
                    }
                }
                Err(e) => {
                    yield Err(e);
                    return;
                }
            }
        }
    };
    Ok(Box::pin(stream))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use futures::stream::BoxStream;

    use super::*;
    use crate::models::ModelCapabilities;
    use crate::provider::ProviderResponse;

    struct StubProvider;

    #[async_trait]
    impl ModelProvider for StubProvider {
        fn provider_name(&self) -> &str {
            "stub"
        }

        fn model_id(&self) -> &str {
            "model"
        }

        fn capabilities(&self) -> &ModelCapabilities {
            static CAPABILITIES: std::sync::OnceLock<ModelCapabilities> =
                std::sync::OnceLock::new();
            CAPABILITIES.get_or_init(ModelCapabilities::default)
        }

        async fn generate_text(
            &self,
            _request: &ProviderRequest,
        ) -> Result<ProviderResponse, RociError> {
            panic!("generate should not be called")
        }

        async fn stream_text(
            &self,
            request: &ProviderRequest,
        ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
            assert_eq!(request.messages[0].text(), "hello");
            assert_eq!(request.settings.max_tokens, Some(80));
            assert!(matches!(
                request.response_format,
                Some(ResponseFormat::JsonObject)
            ));
            assert!(request.tools.is_none());
            Ok(futures::stream::iter(
                [
                    delta(StreamEventType::Start, ""),
                    delta(StreamEventType::TextDelta, "first "),
                    delta(StreamEventType::TextDelta, "second"),
                    delta(StreamEventType::TextDelta, " ignored"),
                    TextStreamDelta {
                        finish_reason: Some(FinishReason::Length),
                        usage: Some(Usage {
                            input_tokens: 2,
                            output_tokens: 4,
                            total_tokens: 6,
                            ..Usage::default()
                        }),
                        ..delta(StreamEventType::Done, "")
                    },
                ]
                .into_iter()
                .map(Ok),
            )
            .boxed())
        }
    }

    fn delta(event_type: StreamEventType, text: &str) -> TextStreamDelta {
        TextStreamDelta {
            text: text.into(),
            event_type,
            tool_call: None,
            finish_reason: None,
            usage: None,
            reasoning: None,
            reasoning_signature: None,
            reasoning_type: None,
        }
    }

    struct StopAfterSecond {
        reset: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl StopCondition for StopAfterSecond {
        async fn reset(&self) {
            self.reset.store(true, std::sync::atomic::Ordering::SeqCst);
        }

        async fn should_stop(&self, text: &str, delta: Option<&str>) -> bool {
            assert!(self.reset.load(std::sync::atomic::Ordering::SeqCst));
            match text {
                "first " => {
                    assert_eq!(delta, Some("first "));
                    false
                }
                "first second" => {
                    assert_eq!(delta, Some("second"));
                    true
                }
                _ => panic!("unexpected accumulated text: {text}"),
            }
        }
    }

    #[tokio::test]
    async fn stream_preserves_provider_events_and_applies_stop_conditions() {
        for stop_early in [false, true] {
            let conditions: Vec<Box<dyn StopCondition>> = if stop_early {
                vec![Box::new(StopAfterSecond {
                    reset: false.into(),
                })]
            } else {
                Vec::new()
            };
            let stream = stream_text(
                Arc::new(StubProvider),
                vec![ModelMessage::user("hello")],
                GenerationSettings {
                    max_tokens: Some(80),
                    response_format: Some(ResponseFormat::JsonObject),
                    ..GenerationSettings::default()
                },
                conditions,
            )
            .await
            .unwrap();
            let deltas: Vec<_> = stream.map(Result::unwrap).collect().await;
            assert_eq!(deltas[0].event_type, StreamEventType::Start);
            assert_eq!(deltas[1].text, "first ");
            assert_eq!(deltas[2].text, "second");
            let final_delta = deltas.last().unwrap();
            assert_eq!(final_delta.event_type, StreamEventType::Done);
            assert!(final_delta.text.is_empty());
            if stop_early {
                assert_eq!(deltas.len(), 4);
                assert_eq!(final_delta.finish_reason, Some(FinishReason::Stop));
                assert!(final_delta.usage.is_none());
            } else {
                assert_eq!(deltas.len(), 5);
                assert_eq!(deltas[3].text, " ignored");
                assert_eq!(final_delta.finish_reason, Some(FinishReason::Length));
                assert_eq!(
                    final_delta.usage,
                    Some(Usage {
                        input_tokens: 2,
                        output_tokens: 4,
                        total_tokens: 6,
                        ..Usage::default()
                    })
                );
            }
        }
    }
}
