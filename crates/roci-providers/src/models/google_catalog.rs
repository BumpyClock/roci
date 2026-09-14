//! Live Gemini API model discovery. Code Assist does not expose a model catalog.

use std::{collections::HashSet, time::Duration};

use roci_core::{
    auth::CredentialMaterial,
    config::RociConfig,
    error::RociError,
    models::{ModelCatalog, ModelCatalogSource, ModelInfo, ModelListOptions, ModelPolicy},
};
use serde::Deserialize;

pub(crate) async fn list_models(
    config: &RociConfig,
    provider: &str,
    options: &ModelListOptions,
) -> Result<ModelCatalog, RociError> {
    if !options.include_dynamic {
        return Ok(ModelCatalog::default());
    }
    let credential = config
        .resolve_provider_credential(provider)?
        .ok_or_else(|| RociError::MissingCredential {
            provider: provider.into(),
        })?;
    let CredentialMaterial::ApiKey(key) = credential.material else {
        // Gemini CLI's Code Assist client exposes quota buckets but no model-list
        // endpoint. Quota bucket IDs are not a complete model catalog.
        return Err(RociError::ModelDiscoveryUnsupported {
            provider: provider.into(),
            reason: "Gemini browser OAuth does not expose model discovery; use an explicit model ID or configure a Gemini API key to list models".into(),
        });
    };
    let base = credential
        .endpoint
        .as_ref()
        .map_or("https://generativelanguage.googleapis.com/v1beta", |url| {
            url.as_str()
        });
    fetch(provider, base, key.expose_secret()).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    models: Vec<Entry>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    name: String,
    display_name: Option<String>,
    input_token_limit: Option<usize>,
    output_token_limit: Option<usize>,
    #[serde(default)]
    supported_generation_methods: Vec<String>,
}

async fn fetch(provider: &str, base: &str, key: &str) -> Result<ModelCatalog, RociError> {
    let mut endpoint =
        reqwest::Url::parse(base).map_err(|_| error("invalid model catalog endpoint"))?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
    {
        return Err(error(
            "model catalog endpoint must be HTTP(S) without embedded credentials",
        ));
    }
    let path = if endpoint.path().trim_matches('/').is_empty() {
        "/v1beta/models".to_owned()
    } else {
        format!("{}/models", endpoint.path().trim_end_matches('/'))
    };
    endpoint.set_path(&path);
    endpoint.set_fragment(None);
    // Query parameters may be required by a configured gateway, but may contain
    // credentials. Keep them on requests and omit them from returned metadata.
    let mut source = endpoint.clone();
    source.set_query(None);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| error("cannot build model catalog client"))?;
    let mut catalog = ModelCatalog::default();
    let mut next = None;
    let mut visited = HashSet::new();
    for _ in 0..100 {
        let mut request = client.get(endpoint.clone()).header("x-goog-api-key", key);
        if let Some(cursor) = &next {
            request = request.query(&[("pageToken", cursor)]);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| error("model catalog request failed"))?;
        if !response.status().is_success() {
            return Err(RociError::api(
                response.status().as_u16(),
                "Gemini model catalog request failed",
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| error("model catalog body failed"))?
        {
            if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                return Err(error("model catalog response exceeds size limit"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let page: Page =
            serde_json::from_slice(&bytes).map_err(|_| error("invalid model catalog response"))?;
        for entry in page.models {
            if !entry
                .supported_generation_methods
                .iter()
                .any(|method| method == "generateContent")
            {
                continue;
            }
            let id = entry
                .name
                .strip_prefix("models/")
                .unwrap_or(&entry.name)
                .to_owned();
            if id.trim().is_empty() {
                return Err(error("model catalog contains an empty model ID"));
            }
            let model: super::google::GoogleModel = id
                .parse()
                .unwrap_or_else(|_| super::google::GoogleModel::Custom(id.clone()));
            let mut capabilities = model.capabilities();
            if let Some(limit) = entry.input_token_limit {
                capabilities.context_length = limit;
            }
            if let Some(limit) = entry.output_token_limit {
                capabilities.max_output_tokens = Some(limit);
            }
            catalog.insert(ModelInfo {
                provider_key: provider.into(),
                model_id: id,
                display_name: entry.display_name,
                capabilities,
                policy: ModelPolicy {
                    requires_credentials: true,
                    local: false,
                    deprecated: false,
                    default_for_provider: false,
                },
                source: ModelCatalogSource::Dynamic {
                    endpoint: source.to_string(),
                },
                metadata: Default::default(),
            });
        }
        let Some(cursor) = page.next_page_token.filter(|token| !token.is_empty()) else {
            return Ok(catalog);
        };
        if !visited.insert(cursor.clone()) {
            return Err(error("model catalog pagination repeated a cursor"));
        }
        next = Some(cursor);
    }
    Err(error("model catalog pagination exceeds page limit"))
}

fn error(message: &str) -> RociError {
    RociError::Provider {
        provider: "google".into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests;
