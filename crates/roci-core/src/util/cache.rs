//! Response cache with TTL and LRU eviction.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// A simple TTL + LRU cache for responses.
#[derive(Clone)]
pub struct ResponseCache {
    inner: Arc<RwLock<CacheInner>>,
}

struct CacheInner {
    entries: HashMap<String, CacheEntry>,
    max_entries: usize,
    ttl: Duration,
}

struct CacheEntry {
    value: String,
    inserted_at: Instant,
    last_accessed: Instant,
}

impl ResponseCache {
    pub fn new(max_entries: usize, ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(CacheInner {
                entries: HashMap::new(),
                max_entries,
                ttl,
            })),
        }
    }

    /// Get a cached value by key, returning None if expired or missing.
    pub fn get(&self, key: &str) -> Option<String> {
        let mut inner = self.inner.write().unwrap();
        let ttl = inner.ttl;

        // Check expiry first
        let expired = inner
            .entries
            .get(key)
            .map(|e| e.inserted_at.elapsed() > ttl);
        match expired {
            Some(true) => {
                inner.entries.remove(key);
                None
            }
            Some(false) => {
                let entry = inner.entries.get_mut(key).unwrap();
                entry.last_accessed = Instant::now();
                Some(entry.value.clone())
            }
            None => None,
        }
    }

    /// Insert a value, evicting LRU if at capacity.
    pub fn insert(&self, key: String, value: String) {
        let mut inner = self.inner.write().unwrap();

        // Evict expired entries
        let ttl = inner.ttl;
        inner.entries.retain(|_, e| e.inserted_at.elapsed() <= ttl);

        // Evict LRU if still at capacity
        if inner.entries.len() >= inner.max_entries {
            if let Some(lru_key) = inner
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_accessed)
                .map(|(k, _)| k.clone())
            {
                inner.entries.remove(&lru_key);
            }
        }

        inner.entries.insert(
            key,
            CacheEntry {
                value,
                inserted_at: Instant::now(),
                last_accessed: Instant::now(),
            },
        );
    }

    /// Clear all entries.
    pub fn clear(&self) {
        self.inner.write().unwrap().entries.clear();
    }

    /// Current number of entries.
    pub fn len(&self) -> usize {
        self.inner.read().unwrap().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
