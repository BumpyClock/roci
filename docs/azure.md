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
- Latest documented preview as of 2025-11-16: `2025-04-01-preview`; GA examples still show `2024-06-01`.
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
| `AZURE_OPENAI_API_KEY` | API key for authentication |
| `AZURE_OPENAI_ENDPOINT` | Full resource URL (`https://{resource}.openai.azure.com`) |
| `AZURE_OPENAI_DEPLOYMENT` | Deployment name (used as model ID) |
| `AZURE_OPENAI_API_VERSION` | API version (default `2025-04-01-preview`) |

`RociConfig` does not auto-load Azure env vars; construct the provider directly with values from `std::env::var`.

## Usage Examples

### Basic text generation

```rust
use roci_core::generation::text::generate_text;
use roci_core::types::{ModelMessage, GenerationSettings};
use roci_providers::provider::azure::AzureOpenAiProvider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = AzureOpenAiProvider::new(
        std::env::var("AZURE_OPENAI_ENDPOINT")?,
        std::env::var("AZURE_OPENAI_DEPLOYMENT").unwrap_or("gpt-4o".into()),
        std::env::var("AZURE_OPENAI_API_KEY")?,
        std::env::var("AZURE_OPENAI_API_VERSION")
            .unwrap_or("2025-04-01-preview".into()),
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
        std::env::var("AZURE_OPENAI_ENDPOINT")?,
        "gpt-4o".into(),
        std::env::var("AZURE_OPENAI_API_KEY")?,
        "2025-04-01-preview".into(),
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
3. **Config loading**: map env vars above into provider construction; `RociConfig` is not involved for Azure-specific keys.

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
