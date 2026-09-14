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
use roci_core::provider::{ModelProvider, ProviderRequest, ProviderResponse};

const COPILOT_EDITOR_VERSION: &str = "vscode/1.96.2";
const COPILOT_EDITOR_PLUGIN_VERSION: &str = "copilot-chat/0.26.7";
const COPILOT_INTEGRATION_ID: &str = "vscode-chat";
const COPILOT_USER_AGENT: &str = "GitHubCopilotChat/0.26.7";
const COPILOT_API_VERSION: &str = "2025-04-01";
const MAX_COPILOT_CATALOG_BYTES: usize = 8 * 1024 * 1024;

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
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| RociError::Provider {
        provider: provider_key.to_string(),
        message: "failed to parse Copilot models response".to_string(),
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
    let failure = |message: &str| RociError::Provider {
        provider: provider_key.to_string(),
        message: message.to_string(),
    };
    if api_key.trim().is_empty() {
        return Err(RociError::Authentication(
            "Copilot model discovery requires credentials".to_string(),
        ));
    }
    let mut url =
        reqwest::Url::parse(base_url).map_err(|_| failure("invalid Copilot models endpoint"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(failure("invalid Copilot models endpoint"));
    }
    url.set_path(&format!("{}/models", url.path().trim_end_matches('/')));
    url.set_fragment(None);
    let mut source_url = url.clone();
    source_url.set_query(None);
    let mut headers = copilot_headers();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|_| failure("could not initialize Copilot model discovery"))?;
    let mut response = client
        .get(url)
        .bearer_auth(api_key)
        .headers(headers)
        .send()
        .await
        .map_err(|_| failure("Copilot model discovery request failed"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(RociError::api(
            status.as_u16(),
            "Copilot model discovery returned an HTTP error",
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_COPILOT_CATALOG_BYTES as u64)
    {
        return Err(failure("Copilot models response exceeded size limit"));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| failure("could not read Copilot models response"))?
    {
        if chunk.len() > MAX_COPILOT_CATALOG_BYTES - body.len() {
            return Err(failure("Copilot models response exceeded size limit"));
        }
        body.extend_from_slice(&chunk);
    }
    let body =
        std::str::from_utf8(&body).map_err(|_| failure("Copilot models response was not UTF-8"))?;
    let mut catalog = parse_copilot_models_response(body, provider_key)?;
    catalog.update_models(|model| {
        model.source = ModelCatalogSource::Dynamic {
            endpoint: source_url.to_string(),
        };
    });
    Ok(catalog)
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
    use wiremock::matchers::{header, method, path, query_param};
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
        let server = MockServer::builder().start().await;
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
    async fn list_copilot_models_preserves_query_but_redacts_source() {
        let server = MockServer::builder().start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(query_param("key", "query-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "future-model"}]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let catalog = list_copilot_models(
            "test-token",
            &format!("{}/v1/?key=query-secret#fragment-secret", server.uri()),
            "github-copilot",
        )
        .await
        .unwrap();
        assert_eq!(
            catalog.models()[0].source,
            ModelCatalogSource::Dynamic {
                endpoint: format!("{}/v1/models", server.uri()),
            }
        );
    }

    #[tokio::test]
    async fn list_copilot_models_preserves_status_without_response_secrets() {
        for status in [401, 403, 404, 405, 429, 503] {
            let server = MockServer::builder().start().await;
            Mock::given(method("GET"))
                .and(path("/models"))
                .respond_with(ResponseTemplate::new(status).set_body_string("body-secret"))
                .mount(&server)
                .await;

            let err = list_copilot_models("token-secret", &server.uri(), "github-copilot")
                .await
                .unwrap_err();

            assert!(matches!(err, RociError::Api { status: actual, .. } if actual == status));
            assert!(!format!("{err:?}").contains("secret"));
        }
    }

    #[tokio::test]
    async fn list_copilot_models_does_not_follow_redirects() {
        let server = MockServer::builder().start().await;
        Mock::given(path("/redirected"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/redirected", server.uri())),
            )
            .mount(&server)
            .await;

        let err = list_copilot_models("test-token", &server.uri(), "github-copilot")
            .await
            .unwrap_err();
        assert!(matches!(err, RociError::Api { status: 302, .. }));
    }

    #[tokio::test]
    async fn list_copilot_models_rejects_oversized_and_malformed_responses() {
        for (body, expected) in [
            (
                " ".repeat(MAX_COPILOT_CATALOG_BYTES + 1),
                "exceeded size limit",
            ),
            ("body-secret".to_string(), "failed to parse"),
        ] {
            let server = MockServer::builder().start().await;
            Mock::given(path("/models"))
                .respond_with(ResponseTemplate::new(200).set_body_string(body))
                .mount(&server)
                .await;
            let err = list_copilot_models("test-token", &server.uri(), "github-copilot")
                .await
                .unwrap_err();
            assert!(err.to_string().contains(expected), "{err}");
            assert!(!format!("{err:?}").contains("body-secret"));
        }
    }

    #[tokio::test]
    async fn list_copilot_models_redacts_invalid_urls_and_credentials() {
        for endpoint in [
            "invalid-url-secret",
            "https://user:password-secret@example.invalid",
            "file:///secret",
        ] {
            let err = list_copilot_models("token-secret", endpoint, "github-copilot")
                .await
                .unwrap_err();
            assert!(!format!("{err:?}").contains("secret"));
        }
        let err = list_copilot_models(
            "token-secret\ninvalid-header",
            "https://example.invalid?key=query-secret",
            "github-copilot",
        )
        .await
        .unwrap_err();
        assert!(!format!("{err:?}").contains("secret"));
    }
}
