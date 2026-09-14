//! OAuth flow implementations for built-in providers.

pub mod claude_code;
#[cfg(feature = "cursor")]
pub mod cursor;
pub mod github_copilot;
pub mod openai_codex;
#[cfg(feature = "grok")]
pub mod xai;

pub(crate) mod factory;
pub(crate) mod runtime;

mod backends;

pub use backends::{ClaudeCodeBackend, GitHubCopilotBackend, OpenAiCodexBackend};

#[cfg(feature = "google")]
pub mod gemini;
