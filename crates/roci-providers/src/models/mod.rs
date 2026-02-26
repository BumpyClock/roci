//! Provider-specific model enums.

#[cfg(feature = "openai")]
pub mod openai;

#[cfg(feature = "anthropic")]
pub mod anthropic;

#[cfg(feature = "google")]
pub mod google;

#[cfg(feature = "grok")]
pub mod grok;

#[cfg(feature = "groq")]
pub mod groq;

#[cfg(feature = "mistral")]
pub mod mistral;

#[cfg(feature = "ollama")]
pub mod ollama;

#[cfg(feature = "lmstudio")]
pub mod lmstudio;

#[cfg(feature = "openai-compatible")]
pub mod openai_compatible;
