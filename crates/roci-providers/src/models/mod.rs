//! Provider-specific model enums.

#[cfg(feature = "openai")]
pub mod codex_catalog;

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

pub(crate) mod remote;

#[cfg(feature = "anthropic")]
pub(crate) mod anthropic_catalog;

#[cfg(feature = "google")]
pub(crate) mod google_catalog;
