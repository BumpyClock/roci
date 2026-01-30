//! Configuration system (layered: code > env > credential file).

pub mod auth;

pub use auth::{AuthManager, AuthValue};

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// Global default config (lazy-initialized from env).
static DEFAULT_CONFIG: OnceLock<RociConfig> = OnceLock::new();

/// Layered configuration for Roci.
#[derive(Debug, Clone)]
pub struct RociConfig {
    api_keys: Arc<RwLock<HashMap<String, String>>>,
    base_urls: Arc<RwLock<HashMap<String, String>>>,
}

impl Default for RociConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl RociConfig {
    /// Create empty config.
    pub fn new() -> Self {
        Self {
            api_keys: Arc::new(RwLock::new(HashMap::new())),
            base_urls: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load from environment variables (OPENAI_API_KEY, ANTHROPIC_API_KEY, etc.).
    pub fn from_env() -> Self {
        let _ = dotenvy::dotenv(); // load .env if present, ignore error
        let config = Self::new();

        let env_mappings = [
            ("OPENAI_API_KEY", "openai"),
            ("OPENAI_COMPAT_API_KEY", "openai-compatible"),
            ("ANTHROPIC_API_KEY", "anthropic"),
            ("GOOGLE_API_KEY", "google"),
            ("GEMINI_API_KEY", "google"),
            ("XAI_API_KEY", "grok"),
            ("GROK_API_KEY", "grok"),
            ("GROQ_API_KEY", "groq"),
            ("MISTRAL_API_KEY", "mistral"),
            ("TOGETHER_API_KEY", "together"),
            ("OPENROUTER_API_KEY", "openrouter"),
            ("REPLICATE_API_TOKEN", "replicate"),
        ];

        for (env_var, provider) in &env_mappings {
            if let Ok(key) = std::env::var(env_var) {
                config.set_api_key(provider, key);
            }
        }

        // Base URL overrides
        let url_mappings = [
            ("OPENAI_BASE_URL", "openai"),
            ("OPENAI_COMPAT_BASE_URL", "openai-compatible"),
            ("ANTHROPIC_BASE_URL", "anthropic"),
            ("OLLAMA_BASE_URL", "ollama"),
            ("LMSTUDIO_BASE_URL", "lmstudio"),
        ];

        for (env_var, provider) in &url_mappings {
            if let Ok(url) = std::env::var(env_var) {
                config.set_base_url(provider, url);
            }
        }

        config
    }

    /// Get (or create) the global default config.
    pub fn global() -> &'static RociConfig {
        DEFAULT_CONFIG.get_or_init(Self::from_env)
    }

    pub fn set_api_key(&self, provider: &str, key: String) {
        self.api_keys
            .write()
            .unwrap()
            .insert(provider.to_string(), key);
    }

    pub fn get_api_key(&self, provider: &str) -> Option<String> {
        self.api_keys.read().unwrap().get(provider).cloned()
    }

    pub fn set_base_url(&self, provider: &str, url: String) {
        self.base_urls
            .write()
            .unwrap()
            .insert(provider.to_string(), url);
    }

    pub fn get_base_url(&self, provider: &str) -> Option<String> {
        self.base_urls.read().unwrap().get(provider).cloned()
    }

    /// Check if a provider has credentials configured.
    pub fn has_credentials(&self, provider: &str) -> bool {
        self.api_keys.read().unwrap().contains_key(provider)
    }
}
