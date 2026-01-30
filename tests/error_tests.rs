//! Tests for the error system.

use roci::error::unified::*;
use roci::error::*;

#[test]
fn error_api_creation() {
    let err = RociError::api(404, "Not found");
    assert!(matches!(&err, RociError::Api { status: 404, .. }));
    assert_eq!(err.to_string(), "API error (status 404): Not found");
}

#[test]
fn error_category_classification() {
    assert_eq!(
        RociError::Authentication("bad key".into()).category(),
        ErrorCategory::Authentication
    );
    assert_eq!(
        RociError::RateLimited {
            retry_after_ms: Some(1000)
        }
        .category(),
        ErrorCategory::RateLimit
    );
    assert_eq!(RociError::Timeout(5000).category(), ErrorCategory::Timeout);
    assert_eq!(
        RociError::Configuration("bad config".into()).category(),
        ErrorCategory::Configuration
    );
}

#[test]
fn error_is_retryable() {
    assert!(RociError::RateLimited {
        retry_after_ms: None
    }
    .is_retryable());
    assert!(RociError::Timeout(5000).is_retryable());
    assert!(!RociError::Authentication("bad key".into()).is_retryable());
    assert!(!RociError::Configuration("bad".into()).is_retryable());
}

#[test]
fn error_recovery_suggestions() {
    assert_eq!(
        RociError::Authentication("".into()).recovery_suggestion(),
        RecoverySuggestion::CheckCredentials
    );
    assert_eq!(
        RociError::RateLimited {
            retry_after_ms: None
        }
        .recovery_suggestion(),
        RecoverySuggestion::RetryWithBackoff
    );
    assert_eq!(
        RociError::Timeout(0).recovery_suggestion(),
        RecoverySuggestion::IncreaseTimeout
    );
}

#[test]
fn error_api_401_is_auth() {
    let err = RociError::api(401, "Unauthorized");
    assert_eq!(err.category(), ErrorCategory::Authentication);
    assert!(!err.is_retryable());
}

#[test]
fn error_api_429_is_rate_limit() {
    let err = RociError::api(429, "Too many requests");
    assert_eq!(err.category(), ErrorCategory::RateLimit);
    assert!(err.is_retryable());
}

#[test]
fn error_api_500_is_server() {
    let err = RociError::api(500, "Internal server error");
    assert_eq!(err.category(), ErrorCategory::Server);
    assert!(err.is_retryable());
}
