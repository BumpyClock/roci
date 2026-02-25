//! Tests for utility modules (retry, cache, usage tracking).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use roci::error::RociError;
use roci::types::usage::{Cost, Usage};
use roci::util::cache::ResponseCache;
use roci::util::retry::RetryPolicy;
use roci::util::usage::UsageTracker;

#[tokio::test(start_paused = true)]
async fn retry_policy_retries_retryable_errors_until_success() {
    let policy = RetryPolicy {
        max_attempts: 4,
        initial_backoff: Duration::from_millis(100),
        max_backoff: Duration::from_millis(100),
        multiplier: 2.0,
    };
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_for_task = attempts.clone();

    let task = tokio::spawn(async move {
        policy
            .execute(|| {
                let attempts = attempts_for_task.clone();
                async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt < 2 {
                        Err(RociError::Timeout(100))
                    } else {
                        Ok::<_, RociError>("ok")
                    }
                }
            })
            .await
    });

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    let result = task.await.unwrap();

    assert_eq!(result.unwrap(), "ok");
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn retry_policy_stops_immediately_for_non_retryable_errors() {
    let policy = RetryPolicy {
        max_attempts: 5,
        initial_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(2),
        multiplier: 2.0,
    };
    let attempts = Arc::new(AtomicUsize::new(0));

    let result = policy
        .execute(|| {
            let attempts = attempts.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(RociError::Authentication("bad-key".to_string()))
            }
        })
        .await;

    match result {
        Err(RociError::Authentication(message)) => assert_eq!(message, "bad-key"),
        other => panic!("expected authentication error, got {other:?}"),
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn retry_policy_returns_last_error_when_attempts_are_exhausted() {
    let policy = RetryPolicy {
        max_attempts: 3,
        initial_backoff: Duration::from_millis(50),
        max_backoff: Duration::from_millis(50),
        multiplier: 2.0,
    };
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_for_task = attempts.clone();

    let task = tokio::spawn(async move {
        policy
            .execute(|| {
                let attempts = attempts_for_task.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>(RociError::RateLimited {
                        retry_after_ms: None,
                    })
                }
            })
            .await
    });

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    let result = task.await.unwrap();

    match result {
        Err(RociError::RateLimited { retry_after_ms }) => assert_eq!(retry_after_ms, None),
        other => panic!("expected rate limit error, got {other:?}"),
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn retry_policy_with_zero_attempts_returns_timeout_without_running_operation() {
    let policy = RetryPolicy {
        max_attempts: 0,
        initial_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(1),
        multiplier: 2.0,
    };
    let attempts = Arc::new(AtomicUsize::new(0));

    let result = policy
        .execute(|| {
            let attempts = attempts.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Ok::<_, RociError>(())
            }
        })
        .await;

    match result {
        Err(RociError::Timeout(ms)) => assert_eq!(ms, 0),
        other => panic!("expected timeout error, got {other:?}"),
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
}

#[test]
fn response_cache_insert_and_get_round_trip() {
    let cache = ResponseCache::new(8, Duration::from_secs(60));

    cache.insert("key-a".to_string(), "value-a".to_string());

    assert_eq!(cache.get("key-a"), Some("value-a".to_string()));
    assert_eq!(cache.len(), 1);
    assert!(!cache.is_empty());
}

#[test]
fn response_cache_expires_entries_after_ttl() {
    let cache = ResponseCache::new(4, Duration::from_millis(10));

    cache.insert("key-a".to_string(), "value-a".to_string());
    std::thread::sleep(Duration::from_millis(20));

    assert_eq!(cache.get("key-a"), None);
    assert_eq!(cache.len(), 0);
}

#[test]
fn response_cache_evicts_least_recently_used_entry() {
    let cache = ResponseCache::new(2, Duration::from_secs(1));

    cache.insert("a".to_string(), "value-a".to_string());
    std::thread::sleep(Duration::from_millis(2));
    cache.insert("b".to_string(), "value-b".to_string());
    assert_eq!(cache.get("a"), Some("value-a".to_string()));
    std::thread::sleep(Duration::from_millis(2));
    cache.insert("c".to_string(), "value-c".to_string());

    assert_eq!(cache.get("a"), Some("value-a".to_string()));
    assert_eq!(cache.get("b"), None);
    assert_eq!(cache.get("c"), Some("value-c".to_string()));
    assert_eq!(cache.len(), 2);
}

#[test]
fn response_cache_clear_removes_entries() {
    let cache = ResponseCache::new(8, Duration::from_secs(60));
    cache.insert("a".to_string(), "1".to_string());
    cache.insert("b".to_string(), "2".to_string());

    cache.clear();

    assert_eq!(cache.len(), 0);
    assert!(cache.is_empty());
}

#[test]
fn response_cache_handles_concurrent_access_without_panics() {
    let cache = ResponseCache::new(128, Duration::from_secs(60));
    let thread_count = 8;
    let operations_per_thread = 250;
    let barrier = Arc::new(Barrier::new(thread_count));

    let mut handles = Vec::new();
    for thread_id in 0..thread_count {
        let cache = cache.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            for op in 0..operations_per_thread {
                let key = format!("shared-key-{}", op % 64);
                let value = format!("{thread_id}-{op}");
                cache.insert(key.clone(), value);
                let _ = cache.get(&key);
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    assert!(cache.len() <= 128);
    assert!(!cache.is_empty());
}

#[test]
fn usage_tracker_accumulates_usage_cost_and_generation_count() {
    let tracker = UsageTracker::new();
    let usage_a = Usage {
        input_tokens: 10,
        output_tokens: 20,
        total_tokens: 30,
        cache_read_tokens: Some(2),
        cache_creation_tokens: Some(1),
        reasoning_tokens: None,
    };
    let usage_b = Usage {
        input_tokens: 3,
        output_tokens: 7,
        total_tokens: 10,
        cache_read_tokens: Some(4),
        cache_creation_tokens: None,
        reasoning_tokens: Some(5),
    };
    let cost_a = Cost {
        input_cost: 0.1,
        output_cost: 0.2,
        total_cost: 0.3,
        currency: "USD".to_string(),
    };

    tracker.record(&usage_a, Some(&cost_a));
    tracker.record(&usage_b, None);

    let total_usage = tracker.total_usage();
    let total_cost = tracker.total_cost();

    assert_eq!(total_usage.input_tokens, 13);
    assert_eq!(total_usage.output_tokens, 27);
    assert_eq!(total_usage.total_tokens, 40);
    assert_eq!(total_usage.cache_read_tokens, Some(6));
    assert_eq!(total_usage.cache_creation_tokens, Some(1));
    assert_eq!(total_usage.reasoning_tokens, Some(5));
    assert!((total_cost.input_cost - 0.1).abs() < 1e-12);
    assert!((total_cost.output_cost - 0.2).abs() < 1e-12);
    assert!((total_cost.total_cost - 0.3).abs() < 1e-12);
    assert_eq!(tracker.generation_count(), 2);
}

#[test]
fn usage_tracker_reset_clears_accumulated_state() {
    let tracker = UsageTracker::new();
    let usage = Usage {
        input_tokens: 1,
        output_tokens: 2,
        total_tokens: 3,
        ..Default::default()
    };
    let cost = Cost {
        input_cost: 1.0,
        output_cost: 2.0,
        total_cost: 3.0,
        currency: "USD".to_string(),
    };
    tracker.record(&usage, Some(&cost));

    tracker.reset();

    assert_eq!(tracker.total_usage(), Usage::default());
    assert_eq!(tracker.total_cost(), Cost::default());
    assert_eq!(tracker.generation_count(), 0);
}

#[test]
fn usage_tracker_handles_concurrent_recording() {
    let tracker = UsageTracker::new();
    let thread_count = 8;
    let records_per_thread = 200;
    let barrier = Arc::new(Barrier::new(thread_count));

    let mut handles = Vec::new();
    for _ in 0..thread_count {
        let tracker = tracker.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            let usage = Usage {
                input_tokens: 1,
                output_tokens: 2,
                total_tokens: 3,
                cache_read_tokens: Some(1),
                cache_creation_tokens: Some(1),
                reasoning_tokens: Some(1),
            };
            let cost = Cost {
                input_cost: 0.01,
                output_cost: 0.02,
                total_cost: 0.03,
                currency: "USD".to_string(),
            };
            barrier.wait();
            for _ in 0..records_per_thread {
                tracker.record(&usage, Some(&cost));
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let expected_records = (thread_count * records_per_thread) as u32;
    let total_usage = tracker.total_usage();
    let total_cost = tracker.total_cost();

    assert_eq!(total_usage.input_tokens, expected_records);
    assert_eq!(total_usage.output_tokens, expected_records * 2);
    assert_eq!(total_usage.total_tokens, expected_records * 3);
    assert_eq!(total_usage.cache_read_tokens, Some(expected_records));
    assert_eq!(total_usage.cache_creation_tokens, Some(expected_records));
    assert_eq!(total_usage.reasoning_tokens, Some(expected_records));
    assert_eq!(
        tracker.generation_count(),
        (thread_count * records_per_thread) as u64
    );
    assert!((total_cost.input_cost - (expected_records as f64 * 0.01)).abs() < 1e-6);
    assert!((total_cost.output_cost - (expected_records as f64 * 0.02)).abs() < 1e-6);
    assert!((total_cost.total_cost - (expected_records as f64 * 0.03)).abs() < 1e-6);
}
