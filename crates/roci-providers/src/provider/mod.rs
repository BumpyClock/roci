//! Built-in provider transport implementations.

#[cfg(feature = "openai")]
pub mod openai;
#[cfg(feature = "openai")]
pub mod openai_responses;

#[cfg(feature = "anthropic")]
pub mod anthropic;

#[cfg(feature = "google")]
pub mod google;

#[cfg(feature = "grok")]
pub mod grok;
#[cfg(feature = "groq")]
pub mod groq;
#[cfg(feature = "lmstudio")]
pub mod lmstudio;
#[cfg(feature = "mistral")]
pub mod mistral;
#[cfg(feature = "ollama")]
pub mod ollama;

#[cfg(feature = "anthropic-compatible")]
pub mod anthropic_compatible;
#[cfg(feature = "openai-compatible")]
pub mod github_copilot;
#[cfg(feature = "openai-compatible")]
pub mod openai_compatible;

#[cfg(feature = "azure")]
pub mod azure;
#[cfg(feature = "openrouter")]
pub mod openrouter;
#[cfg(feature = "replicate")]
pub mod replicate;
#[cfg(feature = "together")]
pub mod together;
