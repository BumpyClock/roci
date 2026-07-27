//! GitHub Copilot provider using Copilot auth/config keys.

use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, USER_AGENT};
use std::collections::BTreeMap;
use std::str::FromStr;

use roci_core::error::RociError;
use roci_core::models::capabilities::ModelCapabilities;
use roci_core::models::{ModelCatalog, ModelCatalogSource, ModelInfo, ModelPolicy};
use roci_core::types::TextStreamDelta;

use super::openai_compatible::OpenAiCompatibleProvider;
use crate::models::openai::OpenAiModel;
use roci_core::provider::http::{bearer_headers, shared_client};
use roci_core::provider::{ModelProvider, ProviderRequest, ProviderResponse};

const COPILOT_EDITOR_VERSION: &str = "vscode/1.96.2";
const COPILOT_EDITOR_PLUGIN_VERSION: &str = "copilot-chat/0.26.7";
const COPILOT_INTEGRATION_ID: &str = "vscode-chat";
const COPILOT_USER_AGENT: &str = "GitHubCopilotChat/0.26.7";
const COPILOT_API_VERSION: &str = "2025-04-01";

pub struct GitHubCopilotProvider {
    inner: OpenAiCompatibleProvider,
}

impl GitHubCopilotProvider {
    pub fn new(model_id: String, api_key: String, base_url: String) -> Self {
        let headers = copilot_headers();
        Self {
            inner: OpenAiCompatibleProvider::new_with_headers(model_id, api_key, base_url, headers),
        }
    }
}

pub(crate) fn copilot_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "Editor-Version",
        HeaderValue::from_static(COPILOT_EDITOR_VERSION),
    );
    headers.insert(
        "Editor-Plugin-Version",
        HeaderValue::from_static(COPILOT_EDITOR_PLUGIN_VERSION),
    );
    headers.insert(
        "Copilot-Integration-Id",
        HeaderValue::from_static(COPILOT_INTEGRATION_ID),
    );
    headers.insert(USER_AGENT, HeaderValue::from_static(COPILOT_USER_AGENT));
    headers.insert(
        "X-Github-Api-Version",
        HeaderValue::from_static(COPILOT_API_VERSION),
    );
    headers
}

pub(crate) fn parse_copilot_models_response(
    body: &str,
    provider_key: &str,
) -> Result<ModelCatalog, RociError> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|error| RociError::Provider {
            provider: provider_key.to_string(),
            message: format!("failed to parse Copilot models response: {error}"),
        })?;
    let models = match &value {
        serde_json::Value::Array(models) => models,
        serde_json::Value::Object(object) => object
            .get("data")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| RociError::Provider {
                provider: provider_key.to_string(),
                message: "Copilot models response missing data array".to_string(),
            })?,
        _ => {
            return Err(RociError::Provider {
                provider: provider_key.to_string(),
                message: "Copilot models response must be an array or object".to_string(),
            });
        }
    };

    let mut catalog = ModelCatalog::default();
    for model in models {
        let model_picker_disabled = model
            .get("model_picker_enabled")
            .and_then(serde_json::Value::as_bool)
            == Some(false);
        let policy_disabled = model
            .get("policy")
            .and_then(serde_json::Value::as_object)
            .and_then(|policy| policy.get("state"))
            .and_then(serde_json::Value::as_str)
            == Some("disabled");
        let tool_calls_disabled = model
            .get("capabilities")
            .and_then(serde_json::Value::as_object)
            .and_then(|capabilities| capabilities.get("supports"))
            .and_then(serde_json::Value::as_object)
            .and_then(|supports| supports.get("tool_calls"))
            .and_then(serde_json::Value::as_bool)
            == Some(false);
        if model_picker_disabled || policy_disabled || tool_calls_disabled {
            continue;
        }

        let Some(id) = model
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.trim().is_empty())
        else {
            continue;
        };
        catalog.insert(copilot_model_info(provider_key, id));
    }

    Ok(catalog)
}

fn copilot_model_info(provider_key: &str, model_id: &str) -> ModelInfo {
    let capabilities = OpenAiModel::from_str(model_id)
        .map(|model| model.capabilities())
        .unwrap_or_default();

    ModelInfo {
        provider_key: provider_key.to_string(),
        model_id: model_id.to_string(),
        display_name: Some(model_id.to_string()),
        capabilities,
        policy: ModelPolicy {
            requires_credentials: true,
            local: false,
            deprecated: false,
            default_for_provider: false,
        },
        source: ModelCatalogSource::Dynamic {
            endpoint: "/models".to_string(),
        },
        metadata: BTreeMap::new(),
    }
}

pub(crate) async fn list_copilot_models(
    api_key: &str,
    base_url: &str,
    provider_key: &str,
) -> Result<ModelCatalog, RociError> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut headers = bearer_headers(api_key);
    headers.extend(copilot_headers());
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

    let response = shared_client()
        .get(url)
        .headers(headers)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(RociError::Network)?;
    let status = response.status();
    let body = response.text().await.map_err(RociError::Network)?;
    match status.as_u16() {
        200..=299 => parse_copilot_models_response(&body, provider_key),
        401 | 403 => Err(RociError::Authentication(body)),
        404 | 405 => Err(RociError::UnsupportedOperation(format!(
            "Copilot models endpoint unsupported: status {}",
            status.as_u16()
        ))),
        500..=599 => Err(RociError::api(status.as_u16(), body)),
        other => Err(RociError::api(other, body)),
    }
}

#[async_trait]
impl ModelProvider for GitHubCopilotProvider {
    fn provider_name(&self) -> &str {
        "github-copilot"
    }

    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn capabilities(&self) -> &ModelCapabilities {
        self.inner.capabilities()
    }

    async fn generate_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        self.inner.generate_text(request).await
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        self.inner.stream_text(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn parser_accepts_data_wrapper() {
        let catalog = parse_copilot_models_response(
            r#"{"data":[
                {"id":"gpt-5","model_picker_enabled":true},
                {"id":"copilot-custom","model_picker_enabled":true}
            ]}"#,
            "github-copilot",
        )
        .unwrap();

        let ids = catalog
            .models()
            .iter()
            .map(|model| model.model_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["copilot-custom", "gpt-5"]);
        assert!(matches!(
            catalog.models()[0].source,
            ModelCatalogSource::Dynamic { .. }
        ));
        let gpt5 = catalog
            .models()
            .iter()
            .find(|model| model.model_id == "gpt-5")
            .expect("gpt-5 present");
        assert!(gpt5
            .capabilities
            .supports_reasoning_effort(roci_core::types::ReasoningEffort::Minimal));
    }

    #[test]
    fn parser_accepts_raw_array() {
        let catalog = parse_copilot_models_response(
            r#"[{"id":"gpt-4.1","model_picker_enabled":true}]"#,
            "github-copilot",
        )
        .unwrap();

        assert_eq!(catalog.models()[0].model_id, "gpt-4.1");
    }

    #[test]
    fn parser_filters_nonselectable_models() {
        let catalog = parse_copilot_models_response(
            r#"{"data":[
                {"id":"chamomile","model_picker_enabled":false},
                {"id":"gpt-41-copilot","model_picker_enabled":true,"policy":{"state":"disabled"}},
                {"id":"embedding","model_picker_enabled":true,"capabilities":{"supports":{"tool_calls":false}}},
                {"id":"missing-picker","policy":{"state":"enabled"}},
                {"id":"claude-sonnet-4","model_picker_enabled":true,"policy":{"state":"enabled"}},
                {"id":"gemini-2.5-pro","model_picker_enabled":true,"policy":{"state":"enabled"}},
                {"id":"gpt-4.1","model_picker_enabled":true,"policy":{"state":"enabled"}}
            ]}"#,
            "github-copilot",
        )
        .unwrap();

        let ids = catalog
            .models()
            .iter()
            .map(|model| model.model_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            ids,
            vec![
                "claude-sonnet-4",
                "gemini-2.5-pro",
                "gpt-4.1",
                "missing-picker"
            ]
        );
    }

    #[test]
    fn parser_returns_empty_catalog_when_all_models_are_filtered() {
        let catalog = parse_copilot_models_response(
            r#"{"data":[
                {"id":"chamomile","model_picker_enabled":false},
                {"id":"gpt-41-copilot","model_picker_enabled":true,"policy":{"state":"disabled"}},
                {"id":"embedding","model_picker_enabled":true,"capabilities":{"supports":{"tool_calls":false}}}
            ]}"#,
            "github-copilot",
        )
        .unwrap();

        let ids = catalog
            .models()
            .iter()
            .map(|model| model.model_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, Vec::<&str>::new());
    }

    #[test]
    fn parser_skips_malformed_ids_without_dropping_valid_models() {
        let catalog = parse_copilot_models_response(
            r#"{"data":[
                {"id":"gpt-5","model_picker_enabled":true},
                {"name":"missing","model_picker_enabled":true},
                {"id":42,"model_picker_enabled":true},
                {"id":"   ","model_picker_enabled":true},
                {"id":"claude-sonnet-4","model_picker_enabled":true}
            ]}"#,
            "github-copilot",
        )
        .unwrap();

        let ids = catalog
            .models()
            .iter()
            .map(|model| model.model_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["claude-sonnet-4", "gpt-5"]);
    }

    #[test]
    fn parser_accepts_empty_data_wrapper() {
        let catalog = parse_copilot_models_response(r#"{"data":[]}"#, "github-copilot").unwrap();

        assert!(catalog.models().is_empty());
    }

    #[tokio::test]
    async fn list_copilot_models_parses_dynamic_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("authorization", "Bearer test-token"))
            .and(header("Copilot-Integration-Id", "vscode-chat"))
            .and(header("accept", "application/json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "gpt-5", "model_picker_enabled": true}]
            })))
            .mount(&server)
            .await;

        let catalog = list_copilot_models("test-token", &server.uri(), "github-copilot")
            .await
            .unwrap();

        assert_eq!(catalog.models()[0].model_id, "gpt-5");
    }

    #[tokio::test]
    async fn list_copilot_models_maps_404_to_unsupported() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;

        let err = list_copilot_models("test-token", &server.uri(), "github-copilot")
            .await
            .unwrap_err();

        assert!(matches!(err, RociError::UnsupportedOperation(_)));
    }

    #[tokio::test]
    async fn list_copilot_models_maps_auth_errors_to_authentication() {
        for status in [401, 403] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/models"))
                .respond_with(ResponseTemplate::new(status).set_body_string("auth failed"))
                .mount(&server)
                .await;

            let err = list_copilot_models("bad-token", &server.uri(), "github-copilot")
                .await
                .unwrap_err();

            assert!(matches!(err, RociError::Authentication(_)));
        }
    }

    #[tokio::test]
    async fn list_copilot_models_maps_5xx_to_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
            .mount(&server)
            .await;

        let err = list_copilot_models("test-token", &server.uri(), "github-copilot")
            .await
            .unwrap_err();

        assert!(matches!(err, RociError::Api { status: 503, .. }));
    }
}
