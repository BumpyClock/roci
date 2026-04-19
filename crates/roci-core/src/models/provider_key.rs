//! Typed provider identifiers and alias handling.

/// Canonical provider keys used across model parsing, config, and provider wiring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKey {
    OpenAi,
    Codex,
    Anthropic,
    Google,
    Grok,
    Groq,
    Mistral,
    Ollama,
    LmStudio,
    OpenAiCompatible,
    GitHubCopilot,
    Azure,
}

impl ProviderKey {
    /// Canonical provider key string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Codex => "codex",
            Self::Anthropic => "anthropic",
            Self::Google => "google",
            Self::Grok => "grok",
            Self::Groq => "groq",
            Self::Mistral => "mistral",
            Self::Ollama => "ollama",
            Self::LmStudio => "lmstudio",
            Self::OpenAiCompatible => "openai-compatible",
            Self::GitHubCopilot => "github-copilot",
            Self::Azure => "azure",
        }
    }

    /// Parse user-facing provider aliases into a typed provider key.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "openai" => Some(Self::OpenAi),
            "codex" | "chatgpt" | "openai-codex" | "openai_codex" => Some(Self::Codex),
            "anthropic" | "claude" => Some(Self::Anthropic),
            "google" | "gemini" => Some(Self::Google),
            "grok" | "xai" => Some(Self::Grok),
            "groq" => Some(Self::Groq),
            "mistral" => Some(Self::Mistral),
            "ollama" => Some(Self::Ollama),
            "lmstudio" => Some(Self::LmStudio),
            "openai-compatible" | "openai_compatible" => Some(Self::OpenAiCompatible),
            "github-copilot" | "github_copilot" | "copilot" => Some(Self::GitHubCopilot),
            "azure" | "azure-openai" | "azure_openai" => Some(Self::Azure),
            _ => None,
        }
    }

    /// Canonical and legacy lookup keys used for config map lookups.
    pub const fn lookup_keys(self) -> &'static [&'static str] {
        match self {
            Self::Codex => &["codex", "openai-codex"],
            Self::OpenAiCompatible => &["openai-compatible", "openai_compatible"],
            Self::GitHubCopilot => &["github-copilot", "github_copilot", "copilot"],
            Self::Grok => &["grok", "xai"],
            Self::OpenAi => &["openai"],
            Self::Anthropic => &["anthropic"],
            Self::Google => &["google"],
            Self::Groq => &["groq"],
            Self::Mistral => &["mistral"],
            Self::Ollama => &["ollama"],
            Self::LmStudio => &["lmstudio"],
            Self::Azure => &["azure", "azure-openai"],
        }
    }

    /// Token store key for OAuth-backed providers.
    pub const fn token_store_key(self) -> Option<&'static str> {
        match self {
            Self::Codex => Some("openai-codex"),
            Self::Anthropic => Some("claude-code"),
            Self::GitHubCopilot => Some("github-copilot"),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderKey;

    #[test]
    fn parses_codex_aliases() {
        for alias in ["codex", "chatgpt", "openai-codex", "openai_codex"] {
            assert_eq!(ProviderKey::parse(alias), Some(ProviderKey::Codex));
        }
    }

    #[test]
    fn codex_lookup_has_legacy_key() {
        assert_eq!(ProviderKey::Codex.lookup_keys(), &["codex", "openai-codex"]);
    }

    #[test]
    fn parses_azure_aliases() {
        for alias in ["azure", "azure-openai", "azure_openai"] {
            assert_eq!(
                ProviderKey::parse(alias),
                Some(ProviderKey::Azure),
                "failed to parse alias: {alias}"
            );
        }
    }

    #[test]
    fn azure_as_str() {
        assert_eq!(ProviderKey::Azure.as_str(), "azure");
    }
}
