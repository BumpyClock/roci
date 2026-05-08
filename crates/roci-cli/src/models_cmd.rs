use std::io::Write;
use std::str::FromStr;
use std::sync::Arc;

use roci::agent::{AgentConfig, AgentRuntime};
use roci::agent_loop::RunStatus;
use roci::config::RociConfig;
use roci::models::{LanguageModel, ModelCatalogSource, ModelInfo, ModelListOptions};
use roci::provider::ProviderRegistry;
use roci::types::Role;

use crate::cli::{
    ModelsArgs, ModelsCommands, ModelsListArgs, ModelsSwitchChatSmokeArgs, ModelsSwitchSmokeArgs,
};

pub async fn handle_models(args: ModelsArgs) -> Result<(), Box<dyn std::error::Error>> {
    match args.command {
        ModelsCommands::List(args) => {
            let registry = Arc::new(roci::default_registry());
            let config = RociConfig::from_env();
            let mut stdout = std::io::stdout();
            run_list(args, registry, config, &mut stdout).await?;
        }
        ModelsCommands::SwitchSmoke(args) => {
            let registry = Arc::new(roci::default_registry());
            let config = RociConfig::from_env();
            let mut stdout = std::io::stdout();
            run_switch_smoke(args, registry, config, &mut stdout).await?;
        }
        ModelsCommands::SwitchChatSmoke(args) => {
            let registry = Arc::new(roci::default_registry());
            let config = RociConfig::from_env();
            let mut stdout = std::io::stdout();
            run_switch_chat_smoke(args, registry, config, &mut stdout).await?;
        }
    }
    Ok(())
}

pub(crate) async fn run_list(
    args: ModelsListArgs,
    registry: Arc<ProviderRegistry>,
    config: RociConfig,
    writer: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = ModelListOptions {
        provider_key: args.provider,
        ..ModelListOptions::default()
    };
    let catalog = registry.list_models(&config, &options).await?;

    if args.json {
        writeln!(
            writer,
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "models": catalog.models(),
            }))?
        )?;
        return Ok(());
    }

    writeln!(writer, "PROVIDER\tMODEL\tCONTEXT\tTOOLS\tVISION\tSOURCE")?;
    let mut models = catalog.models().iter().collect::<Vec<_>>();
    models.sort_by(|left, right| {
        left.provider_key
            .cmp(&right.provider_key)
            .then_with(|| left.model_id.cmp(&right.model_id))
    });
    for model in models {
        writeln!(
            writer,
            "{}\t{}\t{}\t{}\t{}\t{}",
            model.provider_key,
            model.model_id,
            model.capabilities.context_length,
            yes_no(model.capabilities.supports_tools),
            yes_no(model.capabilities.supports_vision),
            source_label(model)
        )?;
    }

    Ok(())
}

pub(crate) async fn run_switch_smoke(
    args: ModelsSwitchSmokeArgs,
    registry: Arc<ProviderRegistry>,
    config: RociConfig,
    writer: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let from = LanguageModel::from_str(&args.from)?;
    let to = LanguageModel::from_str(&args.to)?;
    let agent_config = AgentConfig {
        model: from.clone(),
        ..AgentConfig::default()
    };
    let agent = AgentRuntime::new(registry, config, agent_config);

    let before = agent.current_model().await;
    let previous = agent.switch_model(to.clone()).await?;
    let current = agent.current_model().await;

    if args.json {
        writeln!(
            writer,
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "before": before.to_string(),
                "previous": previous.to_string(),
                "current": current.to_string(),
            }))?
        )?;
        return Ok(());
    }

    writeln!(writer, "BEFORE\tPREVIOUS\tCURRENT")?;
    writeln!(writer, "{before}\t{previous}\t{current}")?;
    Ok(())
}

pub(crate) async fn run_switch_chat_smoke(
    args: ModelsSwitchChatSmokeArgs,
    registry: Arc<ProviderRegistry>,
    config: RociConfig,
    writer: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let from = LanguageModel::from_str(&args.from)?;
    let to = LanguageModel::from_str(&args.to)?;
    let agent_config = AgentConfig {
        model: from.clone(),
        ..AgentConfig::default()
    };
    let agent = AgentRuntime::new(registry, config, agent_config);

    let before = agent.current_model().await;
    let previous = agent.switch_model(to).await?;
    let current = agent.current_model().await;
    let result = agent.prompt(args.prompt).await?;
    let response = result
        .messages
        .iter()
        .rev()
        .find(|message| message.role == Role::Assistant)
        .map(|message| message.text())
        .unwrap_or_default();

    if result.status != RunStatus::Completed {
        return Err(format!(
            "switched model prompt did not complete: {:?} {:?}",
            result.status, result.error
        )
        .into());
    }

    if let Some(expected) = &args.expect {
        if !response.contains(expected) {
            return Err(format!("response did not contain expected text: {expected}").into());
        }
    }

    if args.json {
        writeln!(
            writer,
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "before": before.to_string(),
                "previous": previous.to_string(),
                "current": current.to_string(),
                "status": format!("{:?}", result.status),
                "response": response,
            }))?
        )?;
        return Ok(());
    }

    writeln!(writer, "BEFORE\tPREVIOUS\tCURRENT\tSTATUS\tRESPONSE")?;
    writeln!(
        writer,
        "{}\t{}\t{}\t{:?}\t{}",
        before, previous, current, result.status, response
    )?;
    Ok(())
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn source_label(model: &ModelInfo) -> String {
    match &model.source {
        ModelCatalogSource::Static => "static".to_string(),
        ModelCatalogSource::Dynamic { endpoint } => format!("dynamic:{endpoint}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roci::error::RociError;
    use roci::models::{ModelCapabilities, ModelCatalog, ModelInputCapabilities, ModelPolicy};
    use roci::provider::{ModelProvider, ProviderFactory};
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RecordedCall {
        provider_key: String,
        requested_provider: Option<String>,
    }

    #[derive(Default)]
    struct StubFactory {
        calls: Arc<Mutex<Vec<RecordedCall>>>,
    }

    impl StubFactory {
        fn calls(&self) -> Arc<Mutex<Vec<RecordedCall>>> {
            self.calls.clone()
        }
    }

    impl ProviderFactory for StubFactory {
        fn provider_keys(&self) -> &[&str] {
            &["sentinel"]
        }

        fn requires_credentials(&self, _provider_key: &str) -> bool {
            false
        }

        fn list_models<'a>(
            &'a self,
            _config: &'a RociConfig,
            provider_key: &'a str,
            options: &'a ModelListOptions,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<ModelCatalog, RociError>> + Send + 'a>,
        > {
            let calls = self.calls.clone();
            let provider_key = provider_key.to_string();
            let requested_provider = options.provider_key.clone();
            Box::pin(async move {
                calls.lock().expect("calls lock").push(RecordedCall {
                    provider_key: provider_key.clone(),
                    requested_provider,
                });
                Ok(ModelCatalog::from_models([sentinel_model(&provider_key)]))
            })
        }

        fn create(
            &self,
            _config: &RociConfig,
            _provider_key: &str,
            _model_id: &str,
        ) -> Result<Box<dyn ModelProvider>, RociError> {
            panic!("models list must not create providers")
        }
    }

    fn registry_with_stub() -> (Arc<ProviderRegistry>, Arc<Mutex<Vec<RecordedCall>>>) {
        let factory = Arc::new(StubFactory::default());
        let calls = factory.calls();
        let mut registry = ProviderRegistry::new();
        registry.register(factory);
        (Arc::new(registry), calls)
    }

    fn sentinel_model(provider_key: &str) -> ModelInfo {
        ModelInfo {
            provider_key: provider_key.to_string(),
            model_id: "sentinel-model".to_string(),
            display_name: Some("Sentinel Model".to_string()),
            capabilities: ModelCapabilities {
                supports_vision: true,
                supports_tools: true,
                context_length: 123_456,
                input: ModelInputCapabilities::from_vision_support(true),
                ..ModelCapabilities::default()
            },
            policy: ModelPolicy {
                requires_credentials: false,
                local: true,
                deprecated: false,
                default_for_provider: true,
            },
            source: ModelCatalogSource::Dynamic {
                endpoint: "/sentinel/models".to_string(),
            },
            metadata: Default::default(),
        }
    }

    #[tokio::test]
    async fn run_list_json_wraps_models_and_uses_registry_list_models() {
        let (registry, calls) = registry_with_stub();
        let mut output = Vec::new();

        run_list(
            ModelsListArgs {
                provider: Some("sentinel".to_string()),
                json: true,
            },
            registry,
            RociConfig::new().with_token_store(None),
            &mut output,
        )
        .await
        .unwrap();

        let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(json["models"][0]["model_id"], "sentinel-model");
        assert_eq!(json["models"][0]["provider_key"], "sentinel");
        assert_eq!(
            *calls.lock().expect("calls lock"),
            vec![RecordedCall {
                provider_key: "sentinel".to_string(),
                requested_provider: Some("sentinel".to_string()),
            }]
        );
    }

    #[tokio::test]
    async fn run_list_human_output_contains_sentinel_table_values() {
        let (registry, calls) = registry_with_stub();
        let mut output = Vec::new();

        run_list(
            ModelsListArgs {
                provider: None,
                json: false,
            },
            registry,
            RociConfig::new().with_token_store(None),
            &mut output,
        )
        .await
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("PROVIDER\tMODEL\tCONTEXT\tTOOLS\tVISION\tSOURCE"));
        assert!(
            output.contains("sentinel\tsentinel-model\t123456\tyes\tyes\tdynamic:/sentinel/models")
        );
        assert_eq!(
            *calls.lock().expect("calls lock"),
            vec![RecordedCall {
                provider_key: "sentinel".to_string(),
                requested_provider: None,
            }]
        );
    }

    #[tokio::test]
    async fn run_switch_smoke_exercises_runtime_model_switch() {
        let (registry, calls) = registry_with_stub();
        let mut output = Vec::new();

        run_switch_smoke(
            ModelsSwitchSmokeArgs {
                from: "openai:gpt-4o".to_string(),
                to: "openai:gpt-4.1".to_string(),
                json: true,
            },
            registry,
            RociConfig::new().with_token_store(None),
            &mut output,
        )
        .await
        .unwrap();

        let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(json["before"], "openai:gpt-4o");
        assert_eq!(json["previous"], "openai:gpt-4o");
        assert_eq!(json["current"], "openai:gpt-4.1");
        assert!(calls.lock().expect("calls lock").is_empty());
    }

    #[tokio::test]
    async fn explicit_missing_provider_error_bubbles() {
        let (registry, calls) = registry_with_stub();
        let mut output = Vec::new();

        let err = run_list(
            ModelsListArgs {
                provider: Some("missing".to_string()),
                json: false,
            },
            registry,
            RociConfig::new().with_token_store(None),
            &mut output,
        )
        .await
        .unwrap_err();

        let roci_error = err.downcast_ref::<RociError>().expect("expected RociError");
        assert!(
            matches!(roci_error, RociError::ModelNotFound(message) if message.contains("missing"))
        );
        assert!(calls.lock().expect("calls lock").is_empty());
    }
}
