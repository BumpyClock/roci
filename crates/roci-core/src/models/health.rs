//! Model health observations.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::LanguageModel;
#[cfg(feature = "agent")]
use crate::agent_loop::events::FailureCategory;

#[cfg(not(feature = "agent"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureCategory {
    Auth,
    RateLimit,
    Network,
    Server,
    Timeout,
    Provider,
    Canceled,
}

/// Stable key for model health observations.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelHealthKey {
    pub provider: String,
    pub model_id: String,
}

/// Health signal observed during a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthSignal {
    Success {
        key: ModelHealthKey,
        observed_at_ms: u64,
    },
    TransientFailure {
        key: ModelHealthKey,
        category: FailureCategory,
        observed_at_ms: u64,
    },
    NonRetryableFailure {
        key: ModelHealthKey,
        category: FailureCategory,
        observed_at_ms: u64,
    },
    RetryExhausted {
        candidate_index: usize,
        key: ModelHealthKey,
        category: FailureCategory,
        observed_at_ms: u64,
    },
    CandidateAdvanced {
        from_index: usize,
        to_index: usize,
        from: ModelHealthKey,
        to: ModelHealthKey,
        reason: FailureCategory,
        observed_at_ms: u64,
    },
    Canceled {
        key: ModelHealthKey,
        observed_at_ms: u64,
    },
}

/// Current health status for a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelHealthStatus {
    Unknown,
    Healthy,
    Degraded,
    Unhealthy,
}

/// Health snapshot for one model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelHealthSnapshot {
    pub key: ModelHealthKey,
    pub status: ModelHealthStatus,
    pub consecutive_transient_failures: u32,
    pub last_failure_category: Option<FailureCategory>,
    pub last_failure_at_ms: Option<u64>,
    pub last_success_at_ms: Option<u64>,
}

/// Shared process-local registry of latest model health.
#[derive(Debug, Default)]
pub struct SharedModelHealthRegistry {
    snapshots: Mutex<HashMap<ModelHealthKey, ModelHealthSnapshot>>,
}

/// Session-local model health tracker.
#[derive(Debug, Clone)]
pub struct ModelHealthTracker {
    shared: Arc<SharedModelHealthRegistry>,
    snapshots: Arc<Mutex<HashMap<ModelHealthKey, ModelHealthSnapshot>>>,
}

impl ModelHealthKey {
    /// Build a key from a language model.
    pub fn from_model(model: &LanguageModel) -> Self {
        Self {
            provider: model.provider_name().to_string(),
            model_id: model.model_id().to_string(),
        }
    }
}

impl ModelHealthSnapshot {
    fn unknown(key: ModelHealthKey) -> Self {
        Self {
            key,
            status: ModelHealthStatus::Unknown,
            consecutive_transient_failures: 0,
            last_failure_category: None,
            last_failure_at_ms: None,
            last_success_at_ms: None,
        }
    }
}

impl SharedModelHealthRegistry {
    /// Record a snapshot if it is at least as recent as the current snapshot.
    pub fn observe(&self, snapshot: ModelHealthSnapshot) {
        let mut snapshots = self
            .snapshots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match snapshots.get(&snapshot.key) {
            Some(current) if snapshot_time(&snapshot) < snapshot_time(current) => {}
            _ => {
                snapshots.insert(snapshot.key.clone(), snapshot);
            }
        }
    }

    /// Read latest shared snapshot for a model.
    pub fn snapshot(&self, key: &ModelHealthKey) -> ModelHealthSnapshot {
        self.snapshots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(key)
            .cloned()
            .unwrap_or_else(|| ModelHealthSnapshot::unknown(key.clone()))
    }
}

impl ModelHealthTracker {
    /// Start a fresh health session backed by a shared registry.
    pub fn new_session(shared: Arc<SharedModelHealthRegistry>) -> Self {
        Self {
            shared,
            snapshots: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Observe one health signal.
    pub fn observe(&self, signal: HealthSignal) {
        match signal {
            HealthSignal::Success {
                key,
                observed_at_ms,
            } => self.update_snapshot(key, |snapshot| {
                snapshot.status = ModelHealthStatus::Healthy;
                snapshot.consecutive_transient_failures = 0;
                snapshot.last_success_at_ms = Some(observed_at_ms);
            }),
            HealthSignal::TransientFailure {
                key,
                category,
                observed_at_ms,
            } => self.update_snapshot(key, |snapshot| {
                snapshot.consecutive_transient_failures =
                    snapshot.consecutive_transient_failures.saturating_add(1);
                snapshot.last_failure_category = Some(category);
                snapshot.last_failure_at_ms = Some(observed_at_ms);
                snapshot.status = match snapshot.consecutive_transient_failures {
                    1 | 2 => ModelHealthStatus::Degraded,
                    _ => ModelHealthStatus::Unhealthy,
                };
            }),
            HealthSignal::RetryExhausted {
                key,
                category,
                observed_at_ms,
                ..
            } => self.update_snapshot(key, |snapshot| {
                snapshot.last_failure_category = Some(category);
                snapshot.last_failure_at_ms = Some(observed_at_ms);
                if is_transient(category) {
                    snapshot.status = ModelHealthStatus::Unhealthy;
                    snapshot.consecutive_transient_failures =
                        snapshot.consecutive_transient_failures.max(3);
                }
            }),
            HealthSignal::NonRetryableFailure {
                key,
                category,
                observed_at_ms,
            } => self.update_snapshot(key, |snapshot| {
                snapshot.last_failure_category = Some(category);
                snapshot.last_failure_at_ms = Some(observed_at_ms);
            }),
            HealthSignal::Canceled {
                key,
                observed_at_ms,
            } => self.update_snapshot(key, |snapshot| {
                snapshot.last_failure_category = Some(FailureCategory::Canceled);
                snapshot.last_failure_at_ms = Some(observed_at_ms);
            }),
            HealthSignal::CandidateAdvanced { from, to, .. } => {
                // Touch both endpoints so session-local snapshots can fall back to shared state.
                let _ = self.snapshot(&from);
                let _ = self.snapshot(&to);
            }
        }
    }

    /// Read latest session snapshot for a model.
    pub fn snapshot(&self, key: &ModelHealthKey) -> ModelHealthSnapshot {
        if let Some(snapshot) = self
            .snapshots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(key)
            .cloned()
        {
            snapshot
        } else {
            self.shared.snapshot(key)
        }
    }

    fn update_snapshot(&self, key: ModelHealthKey, update: impl FnOnce(&mut ModelHealthSnapshot)) {
        let snapshot = {
            let mut snapshots = self
                .snapshots
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let snapshot = snapshots
                .entry(key.clone())
                .or_insert_with(|| ModelHealthSnapshot::unknown(key));
            update(snapshot);
            snapshot.clone()
        };
        self.shared.observe(snapshot);
    }
}

fn snapshot_time(snapshot: &ModelHealthSnapshot) -> u64 {
    snapshot
        .last_success_at_ms
        .unwrap_or(0)
        .max(snapshot.last_failure_at_ms.unwrap_or(0))
}

fn is_transient(category: FailureCategory) -> bool {
    matches!(
        category,
        FailureCategory::RateLimit
            | FailureCategory::Network
            | FailureCategory::Server
            | FailureCategory::Timeout
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> ModelHealthKey {
        ModelHealthKey {
            provider: "openai".to_string(),
            model_id: "gpt-4o".to_string(),
        }
    }

    #[test]
    fn unknown_snapshot_has_no_observation() {
        let registry = SharedModelHealthRegistry::default();
        let snapshot = registry.snapshot(&key());

        assert_eq!(snapshot.status, ModelHealthStatus::Unknown);
        assert_eq!(snapshot.last_failure_at_ms, None);
        assert_eq!(snapshot.last_success_at_ms, None);
    }

    #[test]
    fn transient_failures_degrade_then_unhealthy() {
        let shared = Arc::new(SharedModelHealthRegistry::default());
        let tracker = ModelHealthTracker::new_session(shared);
        let key = key();

        tracker.observe(HealthSignal::TransientFailure {
            key: key.clone(),
            category: FailureCategory::Network,
            observed_at_ms: 1,
        });
        assert_eq!(tracker.snapshot(&key).status, ModelHealthStatus::Degraded);

        tracker.observe(HealthSignal::TransientFailure {
            key: key.clone(),
            category: FailureCategory::Timeout,
            observed_at_ms: 2,
        });
        assert_eq!(tracker.snapshot(&key).status, ModelHealthStatus::Degraded);

        tracker.observe(HealthSignal::TransientFailure {
            key: key.clone(),
            category: FailureCategory::Server,
            observed_at_ms: 3,
        });
        assert_eq!(tracker.snapshot(&key).status, ModelHealthStatus::Unhealthy);
    }

    #[test]
    fn success_resets_transient_failures() {
        let shared = Arc::new(SharedModelHealthRegistry::default());
        let tracker = ModelHealthTracker::new_session(shared);
        let key = key();

        tracker.observe(HealthSignal::TransientFailure {
            key: key.clone(),
            category: FailureCategory::Network,
            observed_at_ms: 1,
        });
        tracker.observe(HealthSignal::Success {
            key: key.clone(),
            observed_at_ms: 2,
        });
        let snapshot = tracker.snapshot(&key);

        assert_eq!(snapshot.status, ModelHealthStatus::Healthy);
        assert_eq!(snapshot.consecutive_transient_failures, 0);
        assert_eq!(snapshot.last_success_at_ms, Some(2));
    }

    #[test]
    fn nonretryable_failure_records_without_degrading() {
        let shared = Arc::new(SharedModelHealthRegistry::default());
        let tracker = ModelHealthTracker::new_session(shared);
        let key = key();

        tracker.observe(HealthSignal::NonRetryableFailure {
            key: key.clone(),
            category: FailureCategory::Auth,
            observed_at_ms: 4,
        });
        let snapshot = tracker.snapshot(&key);

        assert_eq!(snapshot.status, ModelHealthStatus::Unknown);
        assert_eq!(snapshot.last_failure_category, Some(FailureCategory::Auth));
        assert_eq!(snapshot.last_failure_at_ms, Some(4));
    }

    #[test]
    fn retry_exhausted_on_transient_marks_unhealthy() {
        let shared = Arc::new(SharedModelHealthRegistry::default());
        let tracker = ModelHealthTracker::new_session(shared);
        let key = key();

        tracker.observe(HealthSignal::RetryExhausted {
            candidate_index: 0,
            key: key.clone(),
            category: FailureCategory::RateLimit,
            observed_at_ms: 5,
        });

        assert_eq!(tracker.snapshot(&key).status, ModelHealthStatus::Unhealthy);
    }

    #[test]
    fn shared_registry_uses_latest_observation() {
        let shared = Arc::new(SharedModelHealthRegistry::default());
        let tracker = ModelHealthTracker::new_session(shared.clone());
        let key = key();

        tracker.observe(HealthSignal::Success {
            key: key.clone(),
            observed_at_ms: 10,
        });
        shared.observe(ModelHealthSnapshot {
            key: key.clone(),
            status: ModelHealthStatus::Unhealthy,
            consecutive_transient_failures: 3,
            last_failure_category: Some(FailureCategory::Server),
            last_failure_at_ms: Some(9),
            last_success_at_ms: None,
        });

        assert_eq!(shared.snapshot(&key).status, ModelHealthStatus::Healthy);
    }

    #[test]
    fn session_snapshot_falls_back_to_shared_snapshot() {
        let shared = Arc::new(SharedModelHealthRegistry::default());
        let key = key();
        shared.observe(ModelHealthSnapshot {
            key: key.clone(),
            status: ModelHealthStatus::Healthy,
            consecutive_transient_failures: 0,
            last_failure_category: None,
            last_failure_at_ms: None,
            last_success_at_ms: Some(10),
        });
        let tracker = ModelHealthTracker::new_session(shared);

        let snapshot = tracker.snapshot(&key);

        assert_eq!(snapshot.status, ModelHealthStatus::Healthy);
        assert_eq!(snapshot.last_success_at_ms, Some(10));
    }

    #[test]
    fn candidate_advanced_preserves_endpoint_snapshots() {
        let shared = Arc::new(SharedModelHealthRegistry::default());
        let tracker = ModelHealthTracker::new_session(shared);
        let from = key();
        let to = ModelHealthKey {
            provider: "anthropic".to_string(),
            model_id: "claude".to_string(),
        };

        tracker.observe(HealthSignal::CandidateAdvanced {
            from_index: 0,
            to_index: 1,
            from: from.clone(),
            to: to.clone(),
            reason: FailureCategory::Timeout,
            observed_at_ms: 7,
        });

        assert_eq!(tracker.snapshot(&from).status, ModelHealthStatus::Unknown);
        assert_eq!(tracker.snapshot(&to).status, ModelHealthStatus::Unknown);
    }
}
