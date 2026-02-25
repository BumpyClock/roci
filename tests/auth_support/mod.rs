#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::Utc;
use roci::auth::{AuthError, Token, TokenStore};

#[derive(Default)]
pub struct InMemoryTokenStore {
    tokens: Mutex<HashMap<(String, String), Token>>,
}

impl InMemoryTokenStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed(&self, provider: &str, profile: &str, token: Token) {
        self.tokens
            .lock()
            .expect("store lock poisoned")
            .insert((provider.to_string(), profile.to_string()), token);
    }

    pub fn get(&self, provider: &str, profile: &str) -> Option<Token> {
        self.tokens
            .lock()
            .expect("store lock poisoned")
            .get(&(provider.to_string(), profile.to_string()))
            .cloned()
    }
}

impl TokenStore for InMemoryTokenStore {
    fn load(&self, provider: &str, profile: &str) -> Result<Option<Token>, AuthError> {
        Ok(self.get(provider, profile))
    }

    fn save(&self, provider: &str, profile: &str, token: &Token) -> Result<(), AuthError> {
        self.tokens
            .lock()
            .expect("store lock poisoned")
            .insert((provider.to_string(), profile.to_string()), token.clone());
        Ok(())
    }

    fn clear(&self, provider: &str, profile: &str) -> Result<(), AuthError> {
        self.tokens
            .lock()
            .expect("store lock poisoned")
            .remove(&(provider.to_string(), profile.to_string()));
        Ok(())
    }
}

pub fn token(access_token: &str) -> Token {
    Token {
        access_token: access_token.to_string(),
        refresh_token: None,
        id_token: None,
        expires_at: None,
        last_refresh: Some(Utc::now()),
        scopes: None,
        account_id: None,
    }
}
