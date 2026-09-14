//! Grok CLI OAuth transport over the xAI Responses SSE endpoint.

use async_trait::async_trait;
use futures::{stream::BoxStream, StreamExt};
use reqwest::header::{HeaderValue, ACCEPT, USER_AGENT};

use roci_core::error::RociError;
use roci_core::models::capabilities::ModelCapabilities;
use roci_core::provider::{ModelProvider, ProviderRequest, ProviderResponse};
use roci_core::types::{ContentPart, StreamEventType, TextStreamDelta, ThinkingContent};

use super::openai_responses::OpenAiResponsesProvider;
use crate::models::{grok::GrokModel, openai::OpenAiModel};

const CLI_BASE_URL: &str = "https://cli-chat-proxy.grok.com/v1";

/// OAuth-only Grok provider. API-key requests continue using `GrokProvider`.
pub struct XaiProvider {
    inner: OpenAiResponsesProvider,
    capabilities: ModelCapabilities,
    cli_endpoint: bool,
}

impl XaiProvider {
    pub fn new(model: GrokModel, access_token: String, base_url: Option<String>) -> Self {
        let base_url = base_url.unwrap_or_else(|| CLI_BASE_URL.into());
        Self {
            cli_endpoint: base_url.trim_end_matches('/') == CLI_BASE_URL,
            capabilities: model.capabilities(),
            inner: OpenAiResponsesProvider::new(
                OpenAiModel::Custom(model.as_str().into()),
                access_token,
                Some(base_url),
                None,
            ),
        }
    }

    fn prepare_request(&self, request: &ProviderRequest) -> Result<ProviderRequest, RociError> {
        if request.settings.speed.is_some() {
            return Err(RociError::UnsupportedOperation(
                "Grok OAuth does not support speed selection".into(),
            ));
        }
        if request
            .settings
            .stop_sequences
            .as_ref()
            .is_some_and(|stops| !stops.is_empty())
        {
            return Err(RociError::UnsupportedOperation(
                "Grok OAuth Responses does not support stop sequences".into(),
            ));
        }
        if request
            .settings
            .openai_responses
            .as_ref()
            .is_some_and(|options| options.previous_response_id.is_some())
        {
            return Err(RociError::UnsupportedOperation(
                "Grok OAuth requires message history instead of previous_response_id".into(),
            ));
        }
        let mut prepared = request.clone();
        // OpenAI's optional proxy environment variable must not reroute an xAI
        // OAuth credential. Explicit xAI base URL configuration is handled above.
        prepared.transport = None;
        let options = prepared
            .settings
            .openai_responses
            .get_or_insert_with(Default::default);
        options.instructions.get_or_insert_with(String::new);
        options.store.get_or_insert(false);
        prepared
            .headers
            .insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        if self.cli_endpoint {
            for (name, value) in [
                ("x-xai-token-auth", "xai-grok-cli"),
                ("x-grok-client-version", "0.2.120"),
                ("x-grok-client-identifier", "grok-shell"),
                ("x-authenticateresponse", "authenticate-response"),
            ] {
                prepared
                    .headers
                    .insert(name, HeaderValue::from_static(value));
            }
            prepared.headers.insert(
                USER_AGENT,
                HeaderValue::from_static("xai-grok-workspace/0.2.120"),
            );
        }
        if prepared.session_id.is_none() && self.model_id().starts_with("grok-composer-") {
            prepared.session_id = Some(uuid::Uuid::new_v4().to_string());
        }
        if let Some(session_id) = &prepared.session_id {
            let value = HeaderValue::from_str(session_id)
                .map_err(|_| RociError::InvalidArgument("invalid Grok conversation ID".into()))?;
            prepared.headers.insert("x-grok-conv-id", value);
        }
        Ok(prepared)
    }
}

#[async_trait]
impl ModelProvider for XaiProvider {
    fn provider_name(&self) -> &str {
        "grok"
    }
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        let mut stream = self.stream_text(request).await?;
        let mut response = ProviderResponse {
            text: String::new(),
            usage: Default::default(),
            tool_calls: Vec::new(),
            finish_reason: None,
            thinking: Vec::new(),
        };
        let mut reasoning = String::new();
        while let Some(delta) = stream.next().await {
            let delta = delta?;
            response.text.push_str(&delta.text);
            if let Some(tool) = delta.tool_call {
                response.tool_calls.push(tool);
            }
            if let Some(usage) = delta.usage {
                response.usage = usage;
            }
            if delta.finish_reason.is_some() {
                response.finish_reason = delta.finish_reason;
            }
            if let Some(text) = delta.reasoning {
                reasoning.push_str(&text);
            }
        }
        if !reasoning.is_empty() {
            response
                .thinking
                .push(ContentPart::Thinking(ThinkingContent {
                    thinking: reasoning,
                    signature: String::new(),
                }));
        }
        Ok(response)
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        let prepared = self.prepare_request(request)?;
        let mut upstream = self.inner.stream_text(&prepared).await?;
        Ok(Box::pin(async_stream::stream! {
            let mut completed = false;
            while let Some(result) = upstream.next().await {
                match result {
                    Ok(delta) => {
                        if delta.event_type == StreamEventType::Done { completed = true; }
                        yield Ok(delta);
                        if completed { break; }
                    }
                    Err(error) => { yield Err(error); return; }
                }
            }
            if !completed {
                yield Err(RociError::Stream("xAI stream disconnected before response.completed".into()));
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roci_core::provider::ToolDefinition;
    use roci_core::types::{FinishReason, ModelMessage};
    use serde_json::json;
    use wiremock::{
        matchers::{body_partial_json, header, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    fn request() -> ProviderRequest {
        ProviderRequest {
            messages: vec![ModelMessage::user("Hello")],
            settings: Default::default(),
            tools: None,
            response_format: None,
            api_key_override: None,
            headers: Default::default(),
            metadata: Default::default(),
            payload_callback: None,
            session_id: Some("conversation".into()),
            transport: None,
        }
    }

    #[test]
    fn unsupported_controls_are_rejected_instead_of_silently_dropped() {
        let provider = XaiProvider::new(GrokModel::Grok4, "token".into(), None);
        let mut request = request();
        request.settings.speed = Some(roci_core::types::GenerationSpeed::Fast);
        assert!(matches!(
            provider.prepare_request(&request),
            Err(RociError::UnsupportedOperation(_))
        ));
        request.settings.speed = None;
        request.settings.stop_sequences = Some(vec!["stop".into()]);
        assert!(matches!(
            provider.prepare_request(&request),
            Err(RociError::UnsupportedOperation(_))
        ));
        request.settings.stop_sequences = None;
        request.settings.openai_responses = Some(roci_core::types::OpenAiResponsesOptions {
            previous_response_id: Some("response".into()),
            ..Default::default()
        });
        assert!(matches!(
            provider.prepare_request(&request),
            Err(RociError::UnsupportedOperation(_))
        ));
    }

    fn events(values: &[serde_json::Value]) -> String {
        values
            .iter()
            .map(|value| format!("data: {value}\n\n"))
            .collect()
    }

    #[test]
    fn cli_identity_headers_are_scoped_to_cli_endpoint() {
        let cli = XaiProvider::new(GrokModel::Grok4, "token".into(), None);
        let request = cli.prepare_request(&request()).unwrap();
        assert_eq!(request.headers["x-xai-token-auth"], "xai-grok-cli");
        assert_eq!(request.headers["x-grok-client-version"], "0.2.120");
        assert_eq!(request.headers["x-grok-conv-id"], "conversation");
        let custom = XaiProvider::new(
            GrokModel::Grok4,
            "token".into(),
            Some("https://example.test/v1".into()),
        );
        let custom_request = custom.prepare_request(&super::tests::request()).unwrap();
        assert!(!custom_request.headers.contains_key("x-xai-token-auth"));
        assert!(!custom_request.headers.contains_key(USER_AGENT));
    }

    #[tokio::test]
    async fn nonstream_generation_consumes_sse_text_tools_and_usage() {
        let server = MockServer::start().await;
        let provider = XaiProvider::new(GrokModel::Grok4, "oauth-token".into(), Some(server.uri()));
        let mut request = request();
        request.tools = Some(vec![ToolDefinition {
            name: "lookup".into(),
            description: "look up".into(),
            parameters: json!({"type":"object"}),
        }]);
        let tool = json!({"type":"function_call", "id":"item-1", "call_id":"call-1", "name":"lookup", "arguments":"{\"query\":\"test\"}"});
        let body = events(&[
            json!({"type":"response.output_text.delta", "delta":"Hello "}),
            json!({"type":"response.output_text.delta", "delta":"world"}),
            json!({"type":"response.output_item.done", "item":tool}),
            json!({"type":"response.completed", "response":{"status":"completed", "output":[tool], "usage":{"input_tokens":3,"output_tokens":4,"total_tokens":7}}}),
        ]);
        Mock::given(method("POST"))
            .and(path("/responses"))
            .and(header("authorization", "Bearer oauth-token"))
            .and(header("accept", "text/event-stream"))
            .and(header("x-grok-conv-id", "conversation"))
            .and(body_partial_json(
                json!({"model":"grok-4", "stream":true, "prompt_cache_key":"conversation"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
            .expect(1)
            .mount(&server)
            .await;
        let response = provider.generate_text(&request).await.unwrap();
        assert_eq!(response.text, "Hello world");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].arguments, json!({"query":"test"}));
        assert_eq!(response.usage.total_tokens, 7);
        assert_eq!(response.finish_reason, Some(FinishReason::ToolCalls));
    }

    #[tokio::test]
    async fn incomplete_stream_is_an_error_after_partial_text() {
        let server = MockServer::start().await;
        let provider = XaiProvider::new(GrokModel::Grok4, "token".into(), Some(server.uri()));
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                events(&[json!({"type":"response.output_text.delta", "delta":"partial"})]),
                "text/event-stream",
            ))
            .mount(&server)
            .await;
        let mut stream = provider.stream_text(&request()).await.unwrap();
        assert_eq!(stream.next().await.unwrap().unwrap().text, "partial");
        assert!(matches!(
            stream.next().await.unwrap(),
            Err(RociError::Stream(_))
        ));
    }

    #[tokio::test]
    async fn unauthorized_is_returned_before_stream_is_exposed() {
        let server = MockServer::start().await;
        let provider = XaiProvider::new(GrokModel::Grok4, "token".into(), Some(server.uri()));
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(json!({"error":{"message":"expired"}})),
            )
            .mount(&server)
            .await;
        let error = match provider.stream_text(&request()).await {
            Ok(_) => panic!("expected HTTP 401"),
            Err(error) => error,
        };
        assert!(matches!(error, RociError::Api { status: 401, .. }));
    }
}
