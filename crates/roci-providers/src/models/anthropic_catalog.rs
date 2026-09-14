//! Authenticated Anthropic model discovery, including compatible servers.

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
    let base = credential
        .endpoint
        .as_ref()
        .map_or("https://api.anthropic.com/v1", |url| url.as_str());
    match &credential.material {
        CredentialMaterial::ApiKey(key) => fetch(provider, base, key.expose_secret(), false).await,
        CredentialMaterial::OAuth(_) => {
            let session =
                crate::auth::factory::oauth_session(config, "anthropic", Some(&credential))?
                    .ok_or_else(|| RociError::MissingCredential {
                        provider: provider.into(),
                    })?;
            let token = session.token(None).await?;
            match fetch(provider, base, &token.access_token, true).await {
                Err(RociError::Api { status: 401, .. }) => {
                    let token = session.token(Some(&token.access_token)).await?;
                    fetch(provider, base, &token.access_token, true).await
                }
                result => result,
            }
        }
    }
}

#[derive(Deserialize)]
struct Page {
    data: Vec<Entry>,
    #[serde(default)]
    has_more: bool,
    last_id: Option<String>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    display_name: Option<String>,
}

pub(crate) async fn fetch(
    provider: &str,
    base: &str,
    key: &str,
    oauth: bool,
) -> Result<ModelCatalog, RociError> {
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
        "/v1/models".to_owned()
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
    let mut after = None;
    let mut visited = HashSet::new();
    for _ in 0..100 {
        let mut request = client
            .get(endpoint.clone())
            .header("anthropic-version", "2023-06-01");
        request = if oauth {
            request
                .bearer_auth(key)
                .header("anthropic-beta", "oauth-2025-04-20")
        } else {
            request.header("x-api-key", key)
        };
        if let Some(cursor) = &after {
            request = request.query(&[("after_id", cursor)]);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| error("model catalog request failed"))?;
        if !response.status().is_success() {
            return Err(RociError::api(
                response.status().as_u16(),
                "Anthropic model catalog request failed",
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
        for entry in page.data {
            if entry.id.trim().is_empty() {
                return Err(error("model catalog contains an empty model ID"));
            }
            let model: super::anthropic::AnthropicModel = entry
                .id
                .parse()
                .unwrap_or_else(|_| super::anthropic::AnthropicModel::Custom(entry.id.clone()));
            catalog.insert(ModelInfo {
                provider_key: provider.into(),
                model_id: entry.id,
                display_name: entry.display_name,
                capabilities: model.capabilities(),
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
        if !page.has_more {
            return Ok(catalog);
        }
        let cursor = page
            .last_id
            .filter(|id| !id.is_empty())
            .ok_or_else(|| error("model catalog pagination cursor missing"))?;
        if !visited.insert(cursor.clone()) {
            return Err(error("model catalog pagination repeated a cursor"));
        }
        after = Some(cursor);
    }
    Err(error("model catalog pagination exceeds page limit"))
}

fn error(message: &str) -> RociError {
    RociError::Provider {
        provider: "anthropic".into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests;
