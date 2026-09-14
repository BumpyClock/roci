//! Provider-specific credential renewal behind the shared OAuth lifecycle.

#![cfg(any(
    feature = "openai",
    feature = "anthropic",
    feature = "google",
    feature = "cursor"
))]

use std::sync::Arc;

use roci_core::auth::{AuthError, CredentialMaterial, ResolvedProviderCredential, Token};
use roci_core::config::RociConfig;
use roci_core::error::RociError;
use roci_core::provider::ModelProvider;

use super::runtime::{Build, ManagedOAuthProvider, OAuthSession, Refresh};

pub(crate) fn managed_provider(
    config: &RociConfig,
    provider: &str,
    model: &str,
    credential: Option<&ResolvedProviderCredential>,
) -> Result<Option<Box<dyn ModelProvider>>, RociError> {
    let Some((seed, session, build)) = setup(config, provider, model, credential)? else {
        return Ok(None);
    };
    Ok(Some(ManagedOAuthProvider::wrap(&seed, session, build)?))
}

/// Use the same account-scoped renewal lifecycle for authenticated discovery.
#[cfg(any(feature = "openai", feature = "anthropic", feature = "cursor"))]
pub(crate) fn oauth_session(
    config: &RociConfig,
    provider: &str,
    credential: Option<&ResolvedProviderCredential>,
) -> Result<Option<OAuthSession>, RociError> {
    Ok(setup(config, provider, "", credential)?.map(|(_, session, _)| session))
}

#[cfg(feature = "openai")]
pub(crate) async fn codex_models(
    config: &RociConfig,
    options: &roci_core::models::ModelListOptions,
) -> Result<roci_core::models::ModelCatalog, RociError> {
    if !options.include_dynamic {
        return Ok(roci_core::models::ModelCatalog::new());
    }
    let credential = config
        .resolve_provider_credential("codex")?
        .ok_or_else(|| RociError::MissingCredential {
            provider: "codex".into(),
        })?;
    let endpoint = credential
        .endpoint
        .as_ref()
        .map_or("https://chatgpt.com/backend-api/codex", |endpoint| {
            endpoint.as_str()
        });
    let account = config.get_account_id("codex");
    match &credential.material {
        CredentialMaterial::ApiKey(key) => {
            crate::models::codex_catalog::fetch(
                endpoint,
                key.expose_secret(),
                account.as_deref(),
                options.include_unavailable,
            )
            .await
        }
        CredentialMaterial::OAuth(_) => {
            let session = oauth_session(config, "codex", Some(&credential))?.ok_or_else(|| {
                RociError::MissingCredential {
                    provider: "codex".into(),
                }
            })?;
            let token = session.token(None).await?;
            let result = crate::models::codex_catalog::fetch(
                endpoint,
                &token.access_token,
                account.as_deref().or(token.account_id.as_deref()),
                options.include_unavailable,
            )
            .await;
            if matches!(result, Err(RociError::Api { status: 401, .. })) {
                let token = session.token(Some(&token.access_token)).await?;
                return crate::models::codex_catalog::fetch(
                    endpoint,
                    &token.access_token,
                    account.as_deref().or(token.account_id.as_deref()),
                    options.include_unavailable,
                )
                .await;
            }
            result
        }
    }
}

#[cfg(any(feature = "cursor", feature = "github-copilot"))]
pub(crate) fn oauth_available(config: &RociConfig, provider: &str) -> Result<bool, RociError> {
    Ok(config.resolve_provider_credential(provider)?.is_some_and(|credential| {
        matches!(credential.material, CredentialMaterial::OAuth(token) if token.is_valid() || token.refresh_token.is_some())
    }))
}

fn setup(
    config: &RociConfig,
    provider: &str,
    model: &str,
    credential: Option<&ResolvedProviderCredential>,
) -> Result<Option<(Token, OAuthSession, Build)>, RociError> {
    let Some(credential) = credential else {
        return Ok(None);
    };
    #[allow(unused_mut)]
    let CredentialMaterial::OAuth(mut seed) = credential.material.clone() else {
        return Ok(None);
    };
    let store = config
        .token_store()
        .cloned()
        .ok_or(AuthError::NotLoggedIn)?;
    let endpoint = credential
        .endpoint
        .as_ref()
        .map(|url| url.as_str().to_owned());
    let model = model.to_owned();
    let (key, refresh, build, derived): (&str, Refresh, Build, bool) = match provider {
        #[cfg(feature = "openai")]
        "codex" => {
            let auth = Arc::new(super::openai_codex::OpenAiCodexAuth::new(store.clone()));
            let account = config.get_account_id("codex");
            (
                "openai-codex",
                Arc::new(move |token| {
                    let auth = auth.clone();
                    Box::pin(async move {
                        auth.refresh_token(&token.ok_or(AuthError::NotLoggedIn)?)
                            .await
                    })
                }),
                Arc::new(move |token| {
                    use std::str::FromStr;
                    let parsed = crate::models::openai::OpenAiModel::from_str(&model)
                        .unwrap_or_else(|_| {
                            crate::models::openai::OpenAiModel::Custom(model.clone())
                        });
                    let endpoint = endpoint
                        .clone()
                        .or_else(|| Some("https://chatgpt.com/backend-api/codex".into()));
                    Ok(Box::new(
                        crate::provider::openai_responses::OpenAiResponsesProvider::new(
                            parsed,
                            token.access_token.clone(),
                            endpoint,
                            account.clone().or_else(|| token.account_id.clone()),
                        ),
                    ))
                }),
                false,
            )
        }
        #[cfg(feature = "google")]
        "google" => {
            let auth = Arc::new(super::gemini::GeminiAuth::new(store.clone()));
            (
                "gemini",
                Arc::new(move |token| {
                    let auth = auth.clone();
                    Box::pin(async move {
                        auth.refresh_token(&token.ok_or(AuthError::NotLoggedIn)?)
                            .await
                    })
                }),
                Arc::new(move |token| {
                    use std::str::FromStr;
                    let model = crate::models::google::GoogleModel::from_str(&model)
                        .unwrap_or_else(|_| {
                            crate::models::google::GoogleModel::Custom(model.clone())
                        });
                    let mut provider =
                        crate::provider::gemini_cli::GeminiCliProvider::new(model, token)?;
                    if let Some(endpoint) = &endpoint {
                        provider = provider.with_endpoint(endpoint);
                    }
                    Ok(Box::new(provider))
                }),
                false,
            )
        }
        #[cfg(feature = "anthropic")]
        "anthropic" => {
            let auth = Arc::new(super::claude_code::ClaudeCodeAuth::new(store.clone()));
            (
                "claude-code",
                Arc::new(move |token| {
                    let auth = auth.clone();
                    Box::pin(async move {
                        auth.refresh_token(&token.ok_or(AuthError::NotLoggedIn)?)
                            .await
                    })
                }),
                Arc::new(move |token| {
                    use std::str::FromStr;
                    let model = crate::models::anthropic::AnthropicModel::from_str(&model)
                        .unwrap_or_else(|_| {
                            crate::models::anthropic::AnthropicModel::Custom(model.clone())
                        });
                    Ok(Box::new(
                        crate::provider::anthropic::AnthropicProvider::new_oauth(
                            model,
                            token.access_token.clone(),
                            endpoint.clone(),
                        ),
                    ))
                }),
                false,
            )
        }
        #[cfg(feature = "github-copilot")]
        "github-copilot" => {
            // The primary GitHub credential lives separately from the short-lived API token.
            seed = store
                .load("github-copilot-api", "default")?
                .unwrap_or_else(|| Token {
                    provider_metadata: None,
                    access_token: String::new(),
                    refresh_token: None,
                    id_token: None,
                    expires_at: None,
                    last_refresh: None,
                    scopes: None,
                    account_id: None,
                });
            let refresh_store = store.clone();
            (
                "github-copilot-api",
                Arc::new(move |_| {
                    let store = refresh_store.clone();
                    Box::pin(async move {
                        let exchanged = super::github_copilot::GitHubCopilotAuth::new(store)
                            .exchange_copilot_token()
                            .await?;
                        Ok(Token {
                            provider_metadata: None,
                            access_token: exchanged.token,
                            refresh_token: None,
                            id_token: None,
                            expires_at: Some(exchanged.expires_at),
                            last_refresh: Some(chrono::Utc::now()),
                            scopes: None,
                            account_id: Some(exchanged.base_url),
                        })
                    })
                }),
                Arc::new(move |token| {
                    Ok(Box::new(
                        crate::provider::github_copilot::GitHubCopilotProvider::new(
                            model.clone(),
                            token.access_token.clone(),
                            endpoint
                                .clone()
                                .or_else(|| token.account_id.clone())
                                .unwrap_or_else(|| "https://api.githubcopilot.com".into()),
                        ),
                    ))
                }),
                true,
            )
        }
        #[cfg(feature = "grok")]
        "grok" => {
            let auth = Arc::new(super::xai::XaiAuth::new(store.clone()));
            (
                "xai",
                Arc::new(move |token| {
                    let auth = auth.clone();
                    Box::pin(async move {
                        auth.refresh_token(&token.ok_or(AuthError::NotLoggedIn)?)
                            .await
                    })
                }),
                Arc::new(move |token| {
                    use std::str::FromStr;
                    let model = crate::models::grok::GrokModel::from_str(&model)
                        .unwrap_or_else(|_| crate::models::grok::GrokModel::Custom(model.clone()));
                    Ok(Box::new(crate::provider::xai::XaiProvider::new(
                        model,
                        token.access_token.clone(),
                        endpoint.clone(),
                    )))
                }),
                false,
            )
        }
        #[cfg(feature = "cursor")]
        "cursor" => {
            let auth = Arc::new(super::cursor::CursorAuth::new(store.clone()));
            let account = config.account().to_owned();
            (
                "cursor",
                Arc::new(move |token| {
                    let auth = auth.clone();
                    Box::pin(async move {
                        auth.refresh_token(&token.ok_or(AuthError::NotLoggedIn)?)
                            .await
                    })
                }),
                Arc::new(move |token| {
                    let provider = crate::provider::cursor::CursorProvider::new(
                        model.clone(),
                        token.access_token.clone(),
                        endpoint.clone(),
                    )
                    .with_account_namespace(account.clone());
                    let provider = match token.account_id.as_ref() {
                        Some(account_id) => provider.with_account_id(account_id.clone()),
                        None => provider,
                    };
                    Ok(Box::new(provider))
                }),
                false,
            )
        }
        _ => return Ok(None),
    };
    Ok(Some((
        seed,
        OAuthSession::new(store, key, refresh, derived),
        build,
    )))
}

#[cfg(feature = "github-copilot")]
pub(crate) async fn copilot_credentials(
    config: &RociConfig,
    rejected: Option<&str>,
    credential: Option<&ResolvedProviderCredential>,
) -> Result<Option<(String, String)>, RociError> {
    let Some((_, session, _)) = setup(config, "github-copilot", "", credential)? else {
        return Ok(None);
    };
    let token = session.token(rejected).await?;
    Ok(Some((
        token.access_token,
        credential
            .and_then(|credential| credential.endpoint.as_ref())
            .map(|url| url.as_str().to_owned())
            .or(token.account_id)
            .ok_or_else(|| {
                RociError::Authentication("Copilot exchange omitted its API endpoint".into())
            })?,
    )))
}

#[cfg(feature = "cursor")]
pub struct CursorFactory;

#[cfg(feature = "cursor")]
impl roci_core::provider::ProviderFactory for CursorFactory {
    fn provider_keys(&self) -> &[&str] {
        &["cursor"]
    }
    fn descriptor(&self) -> roci_core::auth::ProviderDescriptor {
        roci_core::auth::ProviderDescriptor::new("cursor", "Cursor", vec![], true)
    }
    fn is_available(&self, config: &RociConfig, _: &str) -> bool {
        oauth_available(config, "cursor").unwrap_or(false)
    }
    fn create(
        &self,
        config: &RociConfig,
        _: &str,
        model: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        managed_provider(
            config,
            "cursor",
            model,
            config.resolve_provider_credential("cursor")?.as_ref(),
        )?
        .ok_or_else(|| RociError::MissingCredential {
            provider: "cursor".into(),
        })
    }
    fn list_models<'a>(
        &'a self,
        config: &'a RociConfig,
        _: &'a str,
        options: &'a roci_core::models::ModelListOptions,
    ) -> futures::future::BoxFuture<'a, Result<roci_core::models::ModelCatalog, RociError>> {
        Box::pin(async move {
            if !options.include_dynamic {
                return Ok(roci_core::models::ModelCatalog::new());
            }
            let credential = config.resolve_provider_credential("cursor")?;
            let endpoint = credential
                .as_ref()
                .and_then(|credential| credential.endpoint.as_ref())
                .map(|url| url.as_str().to_owned());
            let Some(session) = oauth_session(config, "cursor", credential.as_ref())? else {
                return Err(RociError::MissingCredential {
                    provider: "cursor".into(),
                });
            };
            let token = session.token(None).await?;
            let provider = crate::provider::cursor::CursorProvider::new(
                String::new(),
                token.access_token.clone(),
                endpoint.clone(),
            );
            let ids = match provider.list_model_ids().await {
                Err(RociError::Api { status: 401, .. }) => {
                    let token = session.token(Some(&token.access_token)).await?;
                    crate::provider::cursor::CursorProvider::new(
                        String::new(),
                        token.access_token,
                        endpoint.clone(),
                    )
                    .list_model_ids()
                    .await?
                }
                result => result?,
            };
            Ok(crate::provider::cursor::models::catalog(
                &ids,
                provider.capabilities(),
            ))
        })
    }
}
