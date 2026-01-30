//! Retry with exponential backoff and jitter.

use std::future::Future;
use std::time::Duration;

use crate::error::RociError;

/// Retry policy configuration.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of attempts (including the first).
    pub max_attempts: u32,
    /// Initial backoff duration.
    pub initial_backoff: Duration,
    /// Maximum backoff duration.
    pub max_backoff: Duration,
    /// Backoff multiplier.
    pub multiplier: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            multiplier: 2.0,
        }
    }
}

impl RetryPolicy {
    /// Execute an async operation with retry.
    pub async fn execute<F, Fut, T>(&self, mut operation: F) -> Result<T, RociError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, RociError>>,
    {
        let mut backoff = self.initial_backoff;
        let mut last_error = None;

        for attempt in 0..self.max_attempts {
            match operation().await {
                Ok(value) => return Ok(value),
                Err(e) => {
                    if !e.is_retryable() || attempt + 1 >= self.max_attempts {
                        return Err(e);
                    }

                    tracing::warn!(
                        attempt = attempt + 1,
                        max_attempts = self.max_attempts,
                        error = %e,
                        "Retrying after error"
                    );

                    // Jitter: 75%–125% of backoff
                    let jitter_factor = 0.75 + (rand_factor() * 0.5);
                    let sleep_duration = Duration::from_secs_f64(
                        backoff.as_secs_f64() * jitter_factor,
                    );
                    tokio::time::sleep(sleep_duration).await;

                    backoff = Duration::from_secs_f64(
                        (backoff.as_secs_f64() * self.multiplier).min(self.max_backoff.as_secs_f64()),
                    );

                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| RociError::Timeout(0)))
    }
}

/// Simple pseudo-random factor [0, 1) without pulling in rand crate.
fn rand_factor() -> f64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);

    let hash = hasher.finish();
    (hash % 10000) as f64 / 10000.0
}
