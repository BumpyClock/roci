//! CLI entry point for Roci.

use clap::Parser;

/// Roci AI CLI
#[derive(Parser, Debug)]
#[command(name = "roci", version, about = "Roci — Rust AI SDK CLI")]
pub struct Cli {
    /// Model to use (format: provider:model, e.g., openai:gpt-4o)
    #[arg(short, long, default_value = "openai:gpt-4o")]
    pub model: String,

    /// System prompt
    #[arg(short, long)]
    pub system: Option<String>,

    /// Temperature (0.0 - 2.0)
    #[arg(short, long)]
    pub temperature: Option<f64>,

    /// Max tokens
    #[arg(long)]
    pub max_tokens: Option<u32>,

    /// Enable streaming output
    #[arg(long, default_value = "true")]
    pub stream: bool,

    /// User prompt (positional)
    pub prompt: Option<String>,
}

impl Cli {
    /// Parse CLI arguments.
    pub fn parse_args() -> Self {
        Self::parse()
    }
}
