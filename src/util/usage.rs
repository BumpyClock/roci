//! Usage tracking across sessions.

use std::sync::{Arc, RwLock};

use crate::types::usage::{Cost, Usage};

/// Tracks cumulative usage and cost across generations.
#[derive(Clone)]
pub struct UsageTracker {
    inner: Arc<RwLock<UsageTrackerInner>>,
}

struct UsageTrackerInner {
    total_usage: Usage,
    total_cost: Cost,
    generation_count: u64,
}

impl Default for UsageTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl UsageTracker {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(UsageTrackerInner {
                total_usage: Usage::default(),
                total_cost: Cost::default(),
                generation_count: 0,
            })),
        }
    }

    /// Record usage from a generation.
    pub fn record(&self, usage: &Usage, cost: Option<&Cost>) {
        let mut inner = self.inner.write().unwrap();
        inner.total_usage.merge(usage);
        if let Some(c) = cost {
            inner.total_cost.input_cost += c.input_cost;
            inner.total_cost.output_cost += c.output_cost;
            inner.total_cost.total_cost += c.total_cost;
        }
        inner.generation_count += 1;
    }

    /// Get total usage.
    pub fn total_usage(&self) -> Usage {
        self.inner.read().unwrap().total_usage.clone()
    }

    /// Get total cost.
    pub fn total_cost(&self) -> Cost {
        self.inner.read().unwrap().total_cost.clone()
    }

    /// Get number of generations tracked.
    pub fn generation_count(&self) -> u64 {
        self.inner.read().unwrap().generation_count
    }

    /// Reset all tracking.
    pub fn reset(&self) {
        let mut inner = self.inner.write().unwrap();
        inner.total_usage = Usage::default();
        inner.total_cost = Cost::default();
        inner.generation_count = 0;
    }
}
