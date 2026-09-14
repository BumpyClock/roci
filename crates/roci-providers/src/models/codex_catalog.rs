//! The authenticated Codex account catalog, independent of SDK model enums.
//!
//! The wire contract is the native Codex ModelsClient GET /models response.
//! Keep instructions and other unrelated backend metadata out of catalog output.

use std::str::FromStr;
use std::time::Duration;

use roci_core::error::RociError;
use roci_core::models::{
    ModelCapabilities, ModelCatalog, ModelCatalogSource, ModelInfo, ModelInputCapabilities,
    ModelPolicy, ReasoningEffortCapabilities,
};
use roci_core::types::{GenerationSpeed, ReasoningEffort};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Native Codex protocol compatibility version, verified against installed Codex
/// CLI 0.154.0. This selects the server catalog contract, not the Roci version;
/// update deliberately when validating compatibility with a newer Codex CLI.
pub const CLIENT_VERSION: &str = "0.154.0";
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

fn error(message: &str) -> RociError {
    RociError::Provider {
        provider: "codex".into(),
        message: message.into(),
    }
}

/// Fetch this OAuth account's native catalog. Authentication recovery is owned
/// by the caller; HTTP statuses stay typed without disclosing response bodies.
pub async fn fetch(
    base_url: &str,
    token: &str,
    account_id: Option<&str>,
    include_unavailable: bool,
) -> Result<ModelCatalog, RociError> {
    if token.trim().is_empty() {
        return Err(RociError::MissingCredential {
            provider: "codex".into(),
        });
    }
    let mut url =
        reqwest::Url::parse(base_url).map_err(|_| error("invalid Codex catalog endpoint"))?;
    url.set_path(&format!("{}/models", url.path().trim_end_matches('/')));
    let query: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key != "client_version")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.query_pairs_mut()
        .clear()
        .extend_pairs(query)
        .append_pair("client_version", CLIENT_VERSION);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| error("could not create Codex catalog client"))?;
    let mut request = client
        .get(url)
        .bearer_auth(token)
        .header("originator", "codex_cli_rs")
        .header("user-agent", format!("codex_cli_rs/{CLIENT_VERSION}"))
        .header("accept", "application/json");
    if let Some(account) = account_id.filter(|account| !account.is_empty()) {
        request = request.header("chatgpt-account-id", account);
    }
    let mut response = request
        .send()
        .await
        .map_err(|_| error("Codex catalog request failed"))?;
    if !response.status().is_success() {
        return Err(RociError::Api {
            status: response.status().as_u16(),
            message: "Codex catalog request rejected".into(),
            details: None,
            source: None,
        });
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_BODY_BYTES as u64)
    {
        return Err(error("Codex catalog response exceeds size limit"));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| error("Codex catalog response interrupted"))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
            return Err(error("Codex catalog response exceeds size limit"));
        }
        body.extend_from_slice(&chunk);
    }
    parse(&body, include_unavailable)
}

#[derive(Deserialize)]
struct CatalogResponse {
    models: Vec<AccountModel>,
}

#[derive(Deserialize)]
struct AccountModel {
    slug: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    supported_in_api: Option<bool>,
    #[serde(default)]
    priority: Option<i64>,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    max_context_window: Option<u64>,
    #[serde(default)]
    auto_compact_token_limit: Option<u64>,
    #[serde(default)]
    input_modalities: Option<Vec<String>>,
    #[serde(default)]
    supported_reasoning_levels: Vec<EffortOption>,
    #[serde(default)]
    default_reasoning_level: Option<String>,
    #[serde(default)]
    service_tiers: Vec<ServiceTier>,
    #[serde(default)]
    additional_speed_tiers: Vec<String>,
    #[serde(default)]
    default_service_tier: Option<String>,
    #[serde(default)]
    supports_reasoning_summary_parameter: Option<bool>,
    #[serde(default)]
    support_verbosity: Option<bool>,
    #[serde(default)]
    supports_image_detail_original: Option<bool>,
}

#[derive(Deserialize, Serialize)]
struct EffortOption {
    effort: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct ServiceTier {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

impl AccountModel {
    fn hidden(&self) -> bool {
        matches!(self.visibility.as_deref(), Some("hide" | "none"))
    }

    fn into_info(self, default_model: Option<&str>) -> ModelInfo {
        let mut efforts = Vec::new();
        for option in &self.supported_reasoning_levels {
            if let Ok(effort) = ReasoningEffort::from_str(&option.effort) {
                if !efforts.contains(&effort) {
                    efforts.push(effort);
                }
            }
        }
        let default_effort = self
            .default_reasoning_level
            .as_deref()
            .and_then(|effort| ReasoningEffort::from_str(effort).ok())
            .filter(|effort| efforts.contains(effort));
        // The native client's legacy catalog contract defaults missing modality
        // metadata to text+image; a present list (including text-only) wins.
        let vision = self
            .input_modalities
            .as_ref()
            .is_none_or(|m| m.iter().any(|m| m == "image"));
        let mut speeds = vec![GenerationSpeed::Standard];
        if self.service_tiers.iter().any(|tier| tier.id == "priority")
            || self
                .additional_speed_tiers
                .iter()
                .any(|tier| tier == "fast")
        {
            speeds.push(GenerationSpeed::Fast);
        }
        let capabilities = ModelCapabilities {
            supports_vision: vision,
            supports_tools: true,
            supports_streaming: true,
            supports_system_messages: true,
            supports_reasoning: self
                .supported_reasoning_levels
                .iter()
                .any(|e| e.effort != "none"),
            reasoning_effort: ReasoningEffortCapabilities {
                supported: efforts,
                default: default_effort,
            },
            supported_speeds: speeds,
            context_length: self
                .context_window
                .and_then(|value| usize::try_from(value).ok())
                .filter(|value| *value > 0)
                .unwrap_or_default(),
            input: ModelInputCapabilities::from_vision_support(vision),
            ..ModelCapabilities::default()
        };
        let mut metadata = std::collections::BTreeMap::new();
        for (key, value) in [
            ("description", json!(self.description)),
            ("visibility", json!(self.visibility)),
            ("supported_in_api", json!(self.supported_in_api)),
            ("priority", json!(self.priority)),
            ("context_window", json!(self.context_window)),
            ("max_context_window", json!(self.max_context_window)),
            (
                "auto_compact_token_limit",
                json!(self.auto_compact_token_limit),
            ),
            ("input_modalities", json!(self.input_modalities)),
            (
                "supported_reasoning_levels",
                json!(self.supported_reasoning_levels),
            ),
            (
                "default_reasoning_level",
                json!(self.default_reasoning_level),
            ),
            ("service_tiers", json!(self.service_tiers)),
            ("default_service_tier", json!(self.default_service_tier)),
            ("additional_speed_tiers", json!(self.additional_speed_tiers)),
            (
                "supports_reasoning_summary_parameter",
                json!(self.supports_reasoning_summary_parameter),
            ),
            ("support_verbosity", json!(self.support_verbosity)),
            (
                "supports_image_detail_original",
                json!(self.supports_image_detail_original),
            ),
        ] {
            if !value.is_null() {
                metadata.insert(key.into(), value);
            }
        }
        ModelInfo {
            policy: ModelPolicy {
                requires_credentials: true,
                local: false,
                deprecated: false,
                default_for_provider: default_model == Some(self.slug.as_str()),
            },
            display_name: self
                .display_name
                .filter(|name| !name.is_empty())
                .or_else(|| Some(self.slug.clone())),
            provider_key: "codex".into(),
            model_id: self.slug,
            capabilities,
            source: ModelCatalogSource::Dynamic {
                endpoint: format!("codex:GET /models?client_version={CLIENT_VERSION}"),
            },
            metadata,
        }
    }
}

/// Parse native catalog fields without restricting model IDs or unknown options.
/// `supported_in_api` describes API-key eligibility, not ChatGPT OAuth access.
pub fn parse(body: &[u8], include_unavailable: bool) -> Result<ModelCatalog, RociError> {
    let response: CatalogResponse =
        serde_json::from_slice(body).map_err(|_| error("invalid Codex catalog response"))?;
    if response
        .models
        .iter()
        .any(|model| model.slug.trim().is_empty())
    {
        return Err(error("Codex catalog contains an empty model ID"));
    }
    let default = response
        .models
        .iter()
        .filter(|m| !m.hidden())
        .filter_map(|m| m.priority.map(|priority| (priority, m.slug.as_str())))
        .min()
        .map(|(_, slug)| slug.to_owned());
    Ok(ModelCatalog::from_models(
        response
            .models
            .into_iter()
            .filter(|model| include_unavailable || !model.hidden())
            .map(|model| model.into_info(default.as_deref())),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn future_model() -> serde_json::Value {
        json!({
            "slug":"gpt-future-astra-2099", "display_name":"Future Astra", "visibility":"list",
            "supported_in_api":false, "priority":1, "context_window":272000, "max_context_window":872000,
            "default_reasoning_level":"medium", "input_modalities":["text","image","future-modality"],
            "supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"ultra"},{"effort":"future-effort","description":"Unknown SDK option"}],
            "service_tiers":[{"id":"priority","name":"Fast"},{"id":"future-tier","name":"Future"}],
            "supports_reasoning_summary_parameter":true,
            "model_messages":{"instructions_template":"unrelated-private-instructions"}, "new_backend_option":{"enabled":true}
        })
    }

    #[tokio::test]
    async fn fetches_native_account_catalog_with_protocol_query_and_auth() {
        let server = MockServer::builder().start().await;
        Mock::given(method("GET"))
            .and(path("/backend-api/codex/models"))
            .and(query_param("client_version", CLIENT_VERSION))
            .and(query_param("region", "test"))
            .and(header("authorization", "Bearer oauth-test-token"))
            .and(header("chatgpt-account-id", "account-test"))
            .and(header("originator", "codex_cli_rs"))
            .and(header(
                "user-agent",
                format!("codex_cli_rs/{CLIENT_VERSION}"),
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"models":[future_model()]})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let catalog = fetch(
            &format!(
                "{}/backend-api/codex/?region=test&client_version=obsolete",
                server.uri()
            ),
            "oauth-test-token",
            Some("account-test"),
            false,
        )
        .await
        .unwrap();
        let model = &catalog.models()[0];
        assert_eq!(model.model_id, "gpt-future-astra-2099");
        assert_eq!(model.provider_key, "codex");
        assert_eq!(model.capabilities.context_length, 272000);
        assert_eq!(
            model.capabilities.default_reasoning_effort(),
            Some(ReasoningEffort::Medium)
        );
        assert_eq!(
            model.capabilities.reasoning_effort.supported,
            [
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::Ultra
            ]
        );
        assert_eq!(
            model.capabilities.supported_speeds,
            [GenerationSpeed::Standard, GenerationSpeed::Fast]
        );
        assert_eq!(
            model.metadata["supported_reasoning_levels"][3]["effort"],
            "future-effort"
        );
        assert_eq!(model.metadata["service_tiers"][1]["id"], "future-tier");
        assert_eq!(model.metadata["supported_in_api"], false);
        assert!(model.capabilities.supports_vision);
        assert!(model.policy.default_for_provider);
        let output = serde_json::to_string(&catalog).unwrap();
        assert!(!output.contains("unrelated-private-instructions"));
        assert!(!output.contains("oauth-test-token"));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests[0]
                .url
                .query_pairs()
                .filter(|(key, _)| key == "client_version")
                .count(),
            1
        );
    }

    #[test]
    fn respects_visibility_without_using_api_key_eligibility_or_model_whitelists() {
        let bytes = serde_json::to_vec(&json!({"models":[future_model(),
            {"slug":"hidden-model", "visibility":"hide", "priority":0},
            {"slug":"unlisted-model", "visibility":"none"},
            {"slug":"new-visibility-model", "visibility":"future-visibility", "input_modalities":["text"]}
        ]})).unwrap();
        let visible = parse(&bytes, false).unwrap();
        assert_eq!(visible.models().len(), 2);
        assert!(visible
            .models()
            .iter()
            .any(|m| m.model_id == "gpt-future-astra-2099"));
        let text_only = visible
            .models()
            .iter()
            .find(|m| m.model_id == "new-visibility-model")
            .unwrap();
        assert!(!text_only.capabilities.supports_vision);
        assert!(text_only.capabilities.input.image.is_none());
        assert_eq!(parse(&bytes, true).unwrap().models().len(), 4);
    }

    #[test]
    fn unknown_default_effort_is_preserved_without_invalid_sdk_capabilities() {
        let bytes = serde_json::to_vec(&json!({"models":[{"slug":"unknown-future-model",
            "default_reasoning_level":"future-effort", "supported_reasoning_levels":[{"effort":"future-effort"}]
        }]})).unwrap();
        let catalog = parse(&bytes, false).unwrap();
        let model = &catalog.models()[0];
        assert_eq!(model.metadata["default_reasoning_level"], "future-effort");
        assert_eq!(model.capabilities.default_reasoning_effort(), None);
        assert!(model.capabilities.supports_reasoning);
        assert!(model.capabilities.reasoning_effort.is_valid());
    }

    #[test]
    fn empty_catalog_stays_empty_and_malformed_catalog_is_redacted() {
        assert!(parse(br#"{"models":[]}"#, false)
            .unwrap()
            .models()
            .is_empty());
        for body in [
            br#"{"secret":"secret-value"}"#.as_slice(),
            br#"{"models":[{"slug":""}]}"#,
            br#"{"models":[{"slug":42}]}"#,
            br#"secret-value invalid JSON"#,
        ] {
            let error = parse(body, false).unwrap_err().to_string();
            assert!(!error.contains("secret-value"));
        }
    }

    #[tokio::test]
    async fn http_failures_preserve_status_without_response_body_or_credentials() {
        for status in [401, 403, 429, 500] {
            let server = MockServer::builder().start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(status).set_body_string("secret-backend-body"))
                .expect(1)
                .mount(&server)
                .await;
            let error = fetch(&server.uri(), "secret-oauth-token", None, false)
                .await
                .unwrap_err();
            assert!(matches!(error, RociError::Api { status: actual, .. } if actual == status));
            assert!(!format!("{error:?}").contains("secret-backend-body"));
            assert!(!format!("{error:?}").contains("secret-oauth-token"));
        }
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let destination = MockServer::builder().start().await;
        let server = MockServer::builder().start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", destination.uri()))
            .expect(1)
            .mount(&server)
            .await;
        assert!(matches!(
            fetch(&server.uri(), "secret-oauth-token", None, false).await,
            Err(RociError::Api { status: 302, .. })
        ));
        assert!(destination.received_requests().await.unwrap().is_empty());
    }
}
