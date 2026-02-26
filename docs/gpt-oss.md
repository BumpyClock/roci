# GPT-OSS-120B Integration Guide

GPT-OSS-120B is OpenAI's open-source 120 billion parameter model, designed for high-quality text generation with advanced reasoning capabilities. roci provides integration with this model through both Ollama and LMStudio.

## Overview

GPT-OSS-120B offers:
- **120B parameters** for nuanced understanding
- **128K context window** for long conversations
- **Chain-of-thought reasoning** with multi-channel responses
- **Tool calling** support for function execution
- **Multiple quantizations** from Q4 (65GB) to FP16 (240GB)

## Hardware Requirements

### Minimum Requirements
- **RAM**: 32GB (Q4_0), 64GB (Q4_K_M)
- **GPU**: 8GB VRAM (partial offload)
- **Storage**: 70GB free space
- **CPU**: 8-core modern processor

### Recommended Setup
- **RAM**: 64GB or more
- **GPU**: 24GB VRAM (RTX 3090/4090, M2 Max/Ultra)
- **Storage**: NVMe SSD with 150GB free
- **CPU**: Apple Silicon or recent Intel/AMD

### Optimal Performance
- **RAM**: 128GB
- **GPU**: 48GB VRAM or dual GPUs
- **Storage**: 2TB NVMe SSD
- **Platform**: Apple M2 Ultra or dual RTX 4090

## Installation

### Via Ollama

```bash
# Method 1: Pull pre-built model
ollama pull gpt-oss-120b:q4_k_m

# Method 2: Import from GGUF
ollama create gpt-oss-120b -f ./Modelfile
```

#### Modelfile Configuration
```dockerfile
FROM ./gpt-oss-120b-q4_k_m.gguf

# Model parameters
PARAMETER temperature 0.7
PARAMETER top_p 0.95
PARAMETER top_k 40
PARAMETER num_ctx 32768      # Start with 32K context
PARAMETER num_gpu 999         # Use all available GPU layers
PARAMETER repeat_penalty 1.1
PARAMETER stop "<|endoftext|>"
PARAMETER stop "<|im_end|>"

# System prompt
SYSTEM """
You are GPT-OSS-120B, an advanced language model with reasoning capabilities.
Always structure your responses clearly and think step-by-step.
"""

# Template for conversations
TEMPLATE """{{ if .System }}<|im_start|>system
{{ .System }}<|im_end|>
{{ end }}{{ if .Prompt }}<|im_start|>user
{{ .Prompt }}<|im_end|>
<|im_start|>assistant
{{ end }}"""
```

### Via LMStudio

1. **Download the model**:
   - Open LMStudio
   - Search for "gpt-oss-120b"
   - Select quantization (Q4_K_M recommended)
   - Click Download

2. **Configure settings**:
   ```json
   {
     "context_length": 16384,
     "gpu_layers": -1,
     "temperature": 0.7,
     "top_p": 0.95,
     "repeat_penalty": 1.1,
     "batch_size": 512
   }
   ```

## Usage with roci

### Via Ollama

`OllamaProvider` lives in `crates/roci-providers/src/provider/ollama.rs`.

```rust
use roci_core::generation::text::generate_text;
use roci_core::types::{ModelMessage, GenerationSettings};
use roci_providers::provider::ollama::OllamaProvider;
use roci_providers::models::ollama::OllamaModel;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = OllamaProvider::new(
        OllamaModel::Custom("gpt-oss-120b:q4_k_m".into()),
        "http://localhost:11434".into(),
    );

    let messages = vec![
        ModelMessage::system("You are a helpful assistant."),
        ModelMessage::user("Explain the theory of relativity"),
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

### Via LMStudio

`LmStudioProvider` lives in `crates/roci-providers/src/provider/lmstudio.rs`.

```rust
use roci_core::generation::text::generate_text;
use roci_core::types::{ModelMessage, GenerationSettings};
use roci_providers::provider::lmstudio::LmStudioProvider;
use roci_providers::models::lmstudio::LmStudioModel;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = LmStudioProvider::new(
        LmStudioModel::Custom("gpt-oss-120b".into()),
        "http://localhost:1234".into(),
    );

    let messages = vec![ModelMessage::user("Explain the theory of relativity")];

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

Use roci's `"provider:model_id"` string selector:

```rust
use roci_core::models::LanguageModel;

// Ollama
let model: LanguageModel = "ollama:gpt-oss-120b:q4_k_m".parse()?;

// LMStudio
let model: LanguageModel = "lmstudio:gpt-oss-120b".parse()?;

// Explicit construction
let model = LanguageModel::Known {
    provider_key: "ollama".into(),
    model_id: "gpt-oss-120b:q4_k_m".into(),
};
```

### Streaming Responses

```rust
use futures::StreamExt;
use roci_core::provider::{ModelProvider, ProviderRequest};
use roci_core::types::{ModelMessage, GenerationSettings};
use roci_providers::provider::ollama::OllamaProvider;
use roci_providers::models::ollama::OllamaModel;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = OllamaProvider::new(
        OllamaModel::Custom("gpt-oss-120b:q4_k_m".into()),
        "http://localhost:11434".into(),
    );

    let request = ProviderRequest {
        messages: vec![
            ModelMessage::system("You are a helpful assistant."),
            ModelMessage::user("What would happen if we could travel faster than light?"),
        ],
        settings: GenerationSettings {
            max_tokens: Some(4096),
            temperature: Some(0.8),
            ..Default::default()
        },
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

### Generation with Settings

```rust
use roci_core::generation::text::generate_text;
use roci_core::types::{ModelMessage, GenerationSettings};
use roci_providers::provider::ollama::OllamaProvider;
use roci_providers::models::ollama::OllamaModel;

let provider = OllamaProvider::new(
    OllamaModel::Custom("gpt-oss-120b:q4_k_m".into()),
    "http://localhost:11434".into(),
);

let messages = vec![
    ModelMessage::system("You are a helpful coding assistant."),
    ModelMessage::user("Write a Rust function to compute Fibonacci numbers efficiently."),
];

let settings = GenerationSettings {
    max_tokens: Some(2048),
    temperature: Some(0.3),
    top_p: Some(0.95),
    ..Default::default()
};

let result = generate_text(&provider, messages, settings, &[]).await?;
println!("{}", result.text);
println!("Tokens used: {:?}", result.usage);
```

## Quantization Guide

| Quantization | Size | RAM Required | Quality | Speed | Use Case |
|-------------|------|--------------|---------|-------|----------|
| Q4_0 | 65GB | 32GB | Good | Fast | General use |
| Q4_K_M | 67GB | 32GB | Better | Fast | **Recommended** |
| Q5_K_M | 82GB | 48GB | Very Good | Medium | Quality focus |
| Q6_K | 98GB | 64GB | Excellent | Slower | Research |
| Q8_0 | 127GB | 96GB | Near Perfect | Slow | Maximum quality |
| FP16 | 240GB | 128GB+ | Perfect | Very Slow | Development only |

## Env Var Overrides

roci's `RociConfig::from_env()` reads:

| Variable | Provider | Default |
|---|---|---|
| `OLLAMA_BASE_URL` | ollama | `http://localhost:11434` |
| `LMSTUDIO_BASE_URL` | lmstudio | `http://localhost:1234` |

## Available Ollama Models (Built-in)

`OllamaModel` enum variants:

| Variant | Model ID |
|---|---|
| `OllamaModel::Llama33` | `llama3.3` |
| `OllamaModel::Llama31` | `llama3.1` |
| `OllamaModel::Mistral` | `mistral` |
| `OllamaModel::CodeLlama` | `codellama` |
| `OllamaModel::DeepseekR1` | `deepseek-r1` |
| `OllamaModel::Qwen25` | `qwen2.5` |
| `OllamaModel::Custom(s)` | any string |

## Troubleshooting

### Model Won't Load

```bash
# Check available memory
free -h

# Check Ollama is running
ollama list
curl http://localhost:11434/api/tags
```

Try a smaller context in settings:
```rust
let settings = GenerationSettings {
    max_tokens: Some(4096),
    ..Default::default()
};
```

### Slow Generation

- Enable GPU acceleration: set `num_gpu 999` in Modelfile to offload all layers
- Use streaming so output appears incrementally
- Try a lower quantization (Q4_0 vs Q4_K_M) to reduce memory bandwidth

### Out of Memory

- Use `OllamaModel::Custom` with a smaller variant: `"gpt-oss-120b:q4_0"`
- Reduce context length in Ollama's Modelfile: `PARAMETER num_ctx 8192`
- Close other memory-intensive applications

## Best Practices

1. **Start with smaller context**: Begin with 8K-16K context and increase gradually
2. **Use appropriate quantization**: Q4_K_M offers best quality/performance balance
3. **Monitor memory**: Keep 20% RAM free for system stability
4. **Use streaming**: Local models benefit from streaming for better perceived latency
5. **Batch similar requests**: Ollama handles concurrent requests on the same loaded model

## Advanced Configuration

### Custom Ollama Models

Create specialized variants:
```bash
# High-creativity variant
cat > creative.Modelfile << 'EOF'
FROM gpt-oss-120b:q4_k_m
PARAMETER temperature 1.2
PARAMETER top_p 0.98
PARAMETER repeat_penalty 0.9
EOF

ollama create gpt-oss-creative -f creative.Modelfile
```

Then use with roci:
```rust
let provider = OllamaProvider::new(
    OllamaModel::Custom("gpt-oss-creative".into()),
    "http://localhost:11434".into(),
);
```

## Migration from Cloud Models

### Replacing OpenAI GPT-4

```rust
// Before: cloud model
// let model = "openai:gpt-4o".parse::<LanguageModel>()?;

// After: local model via Ollama (no API key needed)
let provider = OllamaProvider::new(
    OllamaModel::Custom("gpt-oss-120b:q4_k_m".into()),
    "http://localhost:11434".into(),
);
let result = generate_text(&provider, messages, settings, &[]).await?;
```

## Related Documentation

- [LMStudio Integration](lmstudio.md) - Alternative local hosting
- [OpenAI Harmony Features](openai-harmony.md) - Multi-channel responses
