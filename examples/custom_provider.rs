//! Custom provider example — demonstrates implementing `ModelProvider` and
//! `ProviderFactory` for a fictional "EchoProvider" that echoes back input.
//!
//! Shows two usage patterns:
//! 1. **Extend existing registry** — add your provider alongside built-in ones.
//! 2. **Standalone core-only** — use `roci_core` directly with no built-in providers.
//!
//! Run: `cargo run --example custom_provider`

use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use futures::StreamExt;

// Use the meta-crate re-exports (same types as roci_core).
use roci::config::RociConfig;
use roci::error::RociError;
use roci::models::capabilities::ModelCapabilities;
use roci::provider::{
    ModelProvider, ProviderFactory, ProviderRegistry, ProviderRequest, ProviderResponse,
};
use roci::types::{FinishReason, StreamEventType, TextStreamDelta, Usage};

// ---------------------------------------------------------------------------
// 1. Define the provider
// ---------------------------------------------------------------------------

/// A toy provider that echoes user messages back as the "generated" response.
struct EchoProvider {
    model_id: String,
}

#[async_trait]
impl ModelProvider for EchoProvider {
    fn provider_name(&self) -> &str {
        "echo"
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn capabilities(&self) -> &ModelCapabilities {
        // Static capabilities — echo supports nothing fancy.
        static CAPS: ModelCapabilities = ModelCapabilities {
            supports_vision: false,
            supports_tools: false,
            supports_streaming: true,
            supports_json_mode: false,
            supports_json_schema: false,
            supports_reasoning: false,
            supports_system_messages: true,
            context_length: 4096,
            max_output_tokens: None,
        };
        &CAPS
    }

    async fn generate_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        // Collect all user message text and echo it back.
        let echo = collect_user_text(&request.messages);
        Ok(ProviderResponse {
            text: format!("[echo] {echo}"),
            usage: Usage {
                input_tokens: echo.len() as u32,
                output_tokens: echo.len() as u32,
                total_tokens: echo.len() as u32 * 2,
                ..Default::default()
            },
            tool_calls: vec![],
            finish_reason: Some(FinishReason::Stop),
            thinking: vec![],
        })
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        let echo = collect_user_text(&request.messages);
        // Emit one delta per word, then a final Done delta.
        let words: Vec<String> = echo.split_whitespace().map(|w| format!("{w} ")).collect();

        let text_deltas = words.into_iter().map(|word| {
            Ok(TextStreamDelta {
                text: word,
                event_type: StreamEventType::TextDelta,
                tool_call: None,
                finish_reason: None,
                usage: None,
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            })
        });

        let done = std::iter::once(Ok(TextStreamDelta {
            text: String::new(),
            event_type: StreamEventType::Done,
            tool_call: None,
            finish_reason: Some(FinishReason::Stop),
            usage: Some(Usage {
                input_tokens: echo.len() as u32,
                output_tokens: echo.len() as u32,
                total_tokens: echo.len() as u32 * 2,
                ..Default::default()
            }),
            reasoning: None,
            reasoning_signature: None,
            reasoning_type: None,
        }));

        Ok(stream::iter(text_deltas.chain(done)).boxed())
    }
}

/// Extract text from user messages in a request.
fn collect_user_text(messages: &[roci::types::ModelMessage]) -> String {
    use roci::types::{ContentPart, Role};
    messages
        .iter()
        .filter(|m| m.role == Role::User)
        .flat_map(|m| m.content.iter())
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// 2. Define the factory
// ---------------------------------------------------------------------------

/// Factory that creates `EchoProvider` instances for the "echo" provider key.
struct EchoFactory;

impl ProviderFactory for EchoFactory {
    fn provider_keys(&self) -> &[&str] {
        &["echo"]
    }

    fn create(
        &self,
        _config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        Ok(Box::new(EchoProvider {
            model_id: model_id.to_string(),
        }))
    }

    fn parse_model(
        &self,
        _provider_key: &str,
        _model_id: &str,
    ) -> Option<Box<dyn Any + Send + Sync>> {
        // Echo provider accepts any model ID; no parsing needed.
        None
    }
}

// ---------------------------------------------------------------------------
// 3. Wire it up
// ---------------------------------------------------------------------------

#[tokio::main]
#[allow(clippy::result_large_err)]
async fn main() -> roci::error::Result<()> {
    let config = RociConfig::new().with_token_store(None);

    // -- Pattern A: Extend the default registry (built-in providers + echo) --
    println!("=== Pattern A: Extend existing registry ===\n");
    {
        let mut registry = roci::default_registry();
        registry.register(Arc::new(EchoFactory));

        // The echo provider is now available alongside OpenAI, Anthropic, etc.
        assert!(registry.has_provider("echo"));

        let provider = registry.create_provider("echo", "echo-v1", &config)?;
        let response = provider
            .generate_text(&ProviderRequest {
                messages: vec![roci::types::ModelMessage::user(
                    "Hello from the extended registry!",
                )],
                settings: Default::default(),
                tools: None,
                response_format: None,
                session_id: None,
                transport: None,
            })
            .await?;

        println!("Provider: {}", provider.provider_name());
        println!("Model:    {}", provider.model_id());
        println!("Response: {}", response.text);
        println!("Usage:    {} tokens\n", response.usage.total_tokens);
    }

    // -- Pattern B: Standalone core-only registry (no built-in providers) --
    println!("=== Pattern B: Standalone core-only registry ===\n");
    {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(EchoFactory));

        // Only our echo provider is registered.
        assert!(registry.has_provider("echo"));
        assert!(!registry.has_provider("openai"));

        let provider = registry.create_provider("echo", "echo-v2", &config)?;
        let response = roci::generation::generate(provider.as_ref(), "Ping!").await?;
        println!("Response: {response}\n");

        // Streaming works too.
        println!("Streaming:");
        let provider_arc: Arc<dyn ModelProvider> =
            Arc::from(registry.create_provider("echo", "echo-v2", &config)?);
        let mut stream = roci::generation::stream(provider_arc, "Hello streaming world").await?;
        while let Some(delta) = stream.next().await {
            let delta = delta?;
            if !delta.text.is_empty() {
                print!("{}", delta.text);
            }
        }
        println!();
    }

    // -- Show that unregistered providers fail with a clear error --
    println!("\n=== Error handling: unregistered provider ===\n");
    {
        let registry = ProviderRegistry::new();
        match registry.create_provider("nonexistent", "model-1", &config) {
            Err(RociError::ModelNotFound(msg)) => {
                println!("Expected error: {msg}");
            }
            Ok(_) => {
                println!("Unexpected: provider was created");
            }
            Err(e) => {
                println!("Unexpected error variant: {e}");
            }
        }
    }

    Ok(())
}
