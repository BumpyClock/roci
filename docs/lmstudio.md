# LMStudio Provider Documentation

LMStudio is a cross-platform desktop application for running large language models locally with a user-friendly interface. roci provides native integration with LMStudio's OpenAI-compatible API server.

## Overview

LMStudio offers:
- **Cross-platform support** on macOS and Linux (Windows builds exist upstream)
- **GUI for model management** with easy downloads
- **OpenAI-compatible API** at `http://localhost:1234/v1`
- **Hardware acceleration** (Metal, CUDA, ROCm)
- **Model quantization** support (GGUF, GGML)
- **Built-in performance monitoring**

## Installation

### Step 1: Install LMStudio

Download from [https://lmstudio.ai](https://lmstudio.ai) or use Homebrew:

```bash
# macOS
brew install --cask lmstudio

# Linux (AppImage)
wget https://releases.lmstudio.ai/linux/LMStudio.AppImage
chmod +x LMStudio.AppImage
./LMStudio.AppImage
```

### Step 2: Download Models

1. Open LMStudio
2. Navigate to "Discover" tab
3. Search for models (e.g., "llama3", "mistral", "codellama")
4. Select quantization and click "Download"

### Step 3: Start Server

1. Go to "Local Server" tab
2. Select your model
3. Configure settings
4. Click "Start Server"

## Configuration

### Server Settings

```json
{
  "host": "localhost",
  "port": 1234,
  "cors": true,
  "verbose": true
}
```

### Model Configuration

In LMStudio's UI or via config file:

```yaml
# Model Settings
context_length: 16384      # Context window size
n_gpu_layers: -1          # -1 for all layers on GPU
n_batch: 512              # Batch size for prompt processing
threads: 8                # CPU threads (if not fully on GPU)
use_mlock: true           # Lock model in RAM
use_mmap: true            # Memory-map model file

# Inference Settings
temperature: 0.7
top_p: 0.95
top_k: 40
repeat_penalty: 1.1
```

## Usage with roci

### Provider API

`LmStudioProvider` lives in `crates/roci-providers/src/provider/lmstudio.rs`.

```rust
use roci_providers::provider::lmstudio::LmStudioProvider;
use roci_providers::models::lmstudio::LmStudioModel;

let provider = LmStudioProvider::new(
    LmStudioModel::Custom("llama3.3".into()),
    "http://localhost:1234".into(),  // base URL (without /v1)
);
```

The provider appends `/v1` to the base URL automatically. No API key is required for local LMStudio.

### Basic Generation

```rust
use roci_core::generation::text::generate_text;
use roci_core::types::{ModelMessage, GenerationSettings};
use roci_providers::provider::lmstudio::LmStudioProvider;
use roci_providers::models::lmstudio::LmStudioModel;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = LmStudioProvider::new(
        LmStudioModel::Custom("llama3.3".into()),
        "http://localhost:1234".into(),
    );

    let messages = vec![ModelMessage::user("Hello, how are you?")];

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

### ModelSelector String Syntax

roci's `ModelSelector` supports a `"provider:model_id"` string syntax:

```rust
use roci_core::models::LanguageModel;

// Parse from string
let model: LanguageModel = "lmstudio:llama3.3".parse()?;
assert_eq!(model.provider_name(), "lmstudio");
assert_eq!(model.model_id(), "llama3.3");

// Explicit construction
let model = LanguageModel::Known {
    provider_key: "lmstudio".into(),
    model_id: "mistral".into(),
};
```

### Streaming

```rust
use futures::StreamExt;
use roci_core::provider::{ModelProvider, ProviderRequest};
use roci_core::types::{ModelMessage, GenerationSettings};
use roci_providers::provider::lmstudio::LmStudioProvider;
use roci_providers::models::lmstudio::LmStudioModel;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = LmStudioProvider::new(
        LmStudioModel::Custom("llama3.3".into()),
        "http://localhost:1234".into(),
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

### Advanced Generation with Settings

```rust
use roci_core::generation::text::generate_text;
use roci_core::types::{ModelMessage, GenerationSettings};
use roci_providers::provider::lmstudio::LmStudioProvider;
use roci_providers::models::lmstudio::LmStudioModel;

let provider = LmStudioProvider::new(
    LmStudioModel::Custom("codellama".into()),
    "http://localhost:1234".into(),
);

let messages = vec![
    ModelMessage::system("You are an expert Rust developer. Generate clean, efficient code."),
    ModelMessage::user("Write a function to sort a vector of structs by a field."),
];

let settings = GenerationSettings {
    max_tokens: Some(2048),
    temperature: Some(0.3),  // lower temperature for code
    top_p: Some(0.95),
    stop_sequences: Some(vec!["```".into()]),
    ..Default::default()
};

let result = generate_text(&provider, messages, settings, &[]).await?;
println!("{}", result.text);
```

### Remote LMStudio Instance

```rust
// Point to LMStudio running on a different machine
let provider = LmStudioProvider::new(
    LmStudioModel::Custom("llama3.3".into()),
    "http://192.168.1.100:1234".into(),
);
```

## Env Var Override

roci's `RociConfig::from_env()` reads `LMSTUDIO_BASE_URL` to override the default base URL:

```bash
export LMSTUDIO_BASE_URL="http://192.168.1.100:1234"
```

The factory will use this URL when constructing `LmStudioProvider` via the registry.

## Model Library

### Recommended Models for LMStudio

| Model | Size | Use Case | Quantization |
|-------|------|----------|--------------|
| Llama-3.3-70B | 35GB | Chat, reasoning | Q4_K_M |
| Mistral-7B | 4GB | Lightweight, fast | Q8_0 |
| CodeLlama-34B | 20GB | Code generation | Q5_K_M |
| Mixtral-8x7B | 24GB | Fast, versatile | Q4_K_S |
| DeepSeek-R1 | varies | Reasoning | Q4_K_M |
| Phi-3-mini | 2GB | Ultra-light | Q4_K_M |

## Capabilities

`LmStudioModel` reports the following capabilities (all custom models):

| Capability | Supported |
|---|---|
| Tool calling | Yes |
| Streaming | Yes |
| JSON mode | Yes |
| JSON schema | No |
| Vision | No |
| Reasoning | No |
| System messages | Yes |
| Context length | 32,768 tokens |

## Troubleshooting

### Connection Issues

Ensure LMStudio is running and the server is started:

```bash
# Verify server is responding
curl http://localhost:1234/v1/models
```

Common causes of connection failures:
1. LMStudio is not running
2. Server is not started (check "Local Server" tab)
3. Port 1234 is blocked by firewall
4. Wrong base URL passed to `LmStudioProvider::new`

### Model Not Responding

- Verify the model ID matches exactly what LMStudio reports at `/v1/models`
- LMStudio serves the currently loaded model; if no model is loaded, requests will fail

## Best Practices

1. **Start LMStudio before your app**: Ensure the server is running
2. **Use appropriate models**: Match model size to your hardware
3. **Monitor memory usage**: Keep 20% RAM free for stability
4. **Enable GPU acceleration**: Dramatically improves performance
5. **Use streaming for UX**: Better perceived responsiveness
6. **Handle errors gracefully**: Network and memory issues can occur

## FAQ

**Q: Can I use multiple models simultaneously?**
A: LMStudio loads one model at a time, but you can switch models by changing the model ID.

**Q: How do I use LMStudio on a different machine?**
A: Change the base URL: `LmStudioProvider::new(model, "http://192.168.1.100:1234".into())`

**Q: Can I fine-tune models in LMStudio?**
A: LMStudio is for inference only. Use other tools for fine-tuning, then import the GGUF.

**Q: What's the maximum context size?**
A: Depends on model and available RAM. The default capability reports 32K; configure LMStudio for more.

## Related Documentation

- [GPT-OSS-120B Guide](gpt-oss.md) - Detailed setup for open-source large models
- [AI SDK Integration](ai-sdk.md) - Advanced integration patterns
