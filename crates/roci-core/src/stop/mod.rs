//! Stop conditions for streaming generation.

use async_trait::async_trait;

/// Trait for conditions that can stop a text stream early.
#[async_trait]
pub trait StopCondition: Send + Sync {
    /// Check if generation should stop given the accumulated text and current delta.
    async fn should_stop(&self, text: &str, delta: Option<&str>) -> bool;

    /// Reset internal state (for reuse across generations).
    async fn reset(&self);
}
