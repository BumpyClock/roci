//! Built-in provider transport implementations.

#[cfg(feature = "openai")]
pub mod openai;
#[cfg(feature = "openai")]
pub(crate) mod openai_errors;
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
#[cfg(feature = "github-copilot")]
pub mod github_copilot;
#[cfg(feature = "openai-compatible-transport")]
pub mod openai_compatible;

#[cfg(feature = "azure")]
pub mod azure;
#[cfg(feature = "openrouter")]
pub mod openrouter;
#[cfg(feature = "together")]
pub mod together;
