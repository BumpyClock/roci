//! Tests for stop conditions.

use roci::stop::*;

#[tokio::test]
async fn string_stop() {
    let stop = StringStop::new("DONE");
    assert!(!stop.should_stop("processing", None).await);
    assert!(stop.should_stop("processing DONE now", None).await);
}

#[tokio::test]
async fn regex_stop() {
    let stop = RegexStop::new(r"```\s*$").unwrap();
    assert!(!stop.should_stop("some text", None).await);
    assert!(stop.should_stop("some text\n```\n", None).await);
}

#[tokio::test]
async fn token_count_stop_estimated() {
    let stop = TokenCountStop::new(10);
    // 10 tokens ≈ 40 chars
    assert!(!stop.should_stop("short", None).await);
    assert!(stop.should_stop(&"x".repeat(44), None).await);
}

#[tokio::test]
async fn timeout_stop_fires() {
    let stop = TimeoutStop::new(std::time::Duration::from_millis(5));
    // First call sets the deadline
    let _ = stop.should_stop("", None).await;
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert!(stop.should_stop("", None).await);
}

#[tokio::test]
async fn timeout_stop_reset() {
    let stop = TimeoutStop::new(std::time::Duration::from_millis(5));
    let _ = stop.should_stop("", None).await;
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert!(stop.should_stop("", None).await);

    stop.reset().await;
    // After reset, deadline is cleared — next should_stop sets a new one
    assert!(!stop.should_stop("", None).await);
}

#[tokio::test]
async fn predicate_stop() {
    let stop = PredicateStop::new(|text: &str| text.contains("exit"));
    assert!(!stop.should_stop("hello world", None).await);
    assert!(stop.should_stop("please exit now", None).await);
}
