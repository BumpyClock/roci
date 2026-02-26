---
summary: "Azure OpenAI provider support in roci"
read_when: "Working on provider plumbing, authentication, or endpoint wiring"
---

# Azure OpenAI Support

## Goals
- Interoperate with Azure OpenAI chat/completions endpoints without requiring an external proxy.
- Match the ergonomics of LangChain/OpenAI SDK Azure helpers: env-var defaults, deployment-centric model identifiers, and automatic header/query shaping.
- Preserve existing OpenAI-compatible behavior for true OpenAI-clone gateways.

## Azure API Reality Check
- Endpoint shape: `POST https://{resource}.openai.azure.com/openai/deployments/{deploymentId}/chat/completions?api-version=YYYY-MM-DD[-preview]`.
- Auth: `api-key` header.
- Current `AzureFactory` default is `api-version=2024-06-01`.
- Breaking changes around data sources and api-version mismatches are common (e.g., `json_schema` needs `>=2024-08-01-preview`).
- Some toolchains hit 404s when they call `/responses` instead of `/chat/completions`; always use `/chat/completions` for Azure.

## Provider API

`AzureOpenAiProvider` lives in `crates/roci-providers/src/provider/azure.rs`.

```rust
use roci_providers::provider::azure::AzureOpenAiProvider;

let provider = AzureOpenAiProvider::new(
    "https://my-resource.openai.azure.com".to_string(), // endpoint
    "gpt-4o".to_string(),                               // deployment
    std::env::var("AZURE_OPENAI_API_KEY").unwrap(),
    "2025-04-01-preview".to_string(),                   // api_version
);
```

Internally the provider builds:
```
{endpoint}/openai/deployments/{deployment}/chat/completions?api-version={api_version}
```
and passes the `api-key` header.

## Configuration Env Vars

| Variable | Purpose |
|---|---|
| `OPENAI_API_KEY` | Used by `AzureFactory` as the Azure API key |
| `OPENAI_BASE_URL` | Used by `AzureFactory` as the Azure endpoint |
| model id (`azure:<deployment>`) | Deployment name passed as `model_id` |
| `api-version` | Hardcoded to `2024-06-01` in `AzureFactory` |

`RociConfig::from_env()` currently loads `OPENAI_API_KEY` and `OPENAI_BASE_URL` (not `AZURE_OPENAI_*` keys). `AzureFactory` reads from those OpenAI mappings.

## Usage Examples

### Basic text generation

```rust
use roci_core::generation::text::generate_text;
use roci_core::types::{ModelMessage, GenerationSettings};
use roci_providers::provider::azure::AzureOpenAiProvider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = AzureOpenAiProvider::new(
        std::env::var("OPENAI_BASE_URL")?,
        std::env::var("AZURE_OPENAI_DEPLOYMENT").unwrap_or("gpt-4o".into()),
        std::env::var("OPENAI_API_KEY")?,
        "2024-06-01".into(),
    );

    let messages = vec![
        ModelMessage::system("You are a helpful assistant."),
        ModelMessage::user("Summarize CCPA in bullet points"),
    ];

    let result = generate_text(
        &provider,
        messages,
        GenerationSettings::default(),
        &[],
    )
    .await?;

    println!("{}", result.text);
    Ok(())
}
```

### Streaming

```rust
use futures::StreamExt;
use roci_core::provider::{ModelProvider, ProviderRequest};
use roci_core::types::{ModelMessage, GenerationSettings};
use roci_providers::provider::azure::AzureOpenAiProvider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = AzureOpenAiProvider::new(
        std::env::var("OPENAI_BASE_URL")?,
        "gpt-4o".into(),
        std::env::var("OPENAI_API_KEY")?,
        "2024-06-01".into(),
    );

    let request = ProviderRequest {
        messages: vec![ModelMessage::user("Explain quantum computing")],
        settings: GenerationSettings::default(),
        tools: None,
        response_format: None,
        session_id: None,
        transport: None,
    };

    let mut stream = provider.stream_text(&request).await?;
    while let Some(delta) = stream.next().await {
        match delta? {
            d if !d.text.is_empty() => print!("{}", d.text),
            _ => {}
        }
    }
    println!();
    Ok(())
}
```

## Wire Construction Rules

- Base URL: provider takes the full endpoint (`https://{resource}.openai.azure.com`).
- Path template: `/openai/deployments/{deployment}/chat/completions?api-version={api_version}`.
- Auth: `api-key` header.
- `Content-Type: application/json` set automatically.

## Integration Points

1. **Provider construction**: `AzureOpenAiProvider::new(endpoint, deployment, api_key, api_version)`.
2. **Inner delegation**: wraps `OpenAiProvider` with the Azure-specific URL pre-built.
3. **Factory wiring**: `AzureFactory` reads API key and base URL via `ProviderKey::OpenAi` config mappings and hardcodes `api_version = "2024-06-01"`.

## Tests

- Unit: URL construction with permutations (endpoint, deployment, api-version).
- Integration (live): `cargo test --test live_providers -- --ignored --nocapture`.
- Regression: ensure OpenAI-compatible providers remain unchanged (no Azure defaults leak).

## Troubleshooting

| Error | Likely cause |
|---|---|
| 401 Unauthorized | Wrong `api-key` header value |
| 404 Not Found | Wrong deployment name or wrong path (should be `/chat/completions`) |
| 400 Bad Request | `api-version` too old for the feature (e.g., `json_schema` needs `>=2024-08-01-preview`) |

## Rollout

- Ship behind a minor version bump; no breaking changes to existing providers.
- Announce deprecation date for proxy-based Azure guidance once native provider is stable.
