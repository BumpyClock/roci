//! OAuth flow implementations for built-in providers.

pub mod claude_code;
pub mod github_copilot;
pub mod openai_codex;

mod backends;

pub use backends::{ClaudeCodeBackend, GitHubCopilotBackend, OpenAiCodexBackend};
