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
fn error_helper_mappings_are_stable_for_major_variants() {
    struct Case {
        error: RociError,
        expected_category: ErrorCategory,
        expected_retryable: bool,
        expected_recovery: RecoverySuggestion,
    }

    let network_error = reqwest::Client::new()
        .get("http://[::1")
        .build()
        .unwrap_err();
    let io_error = std::io::Error::new(std::io::ErrorKind::Other, "disk");
    let serde_error = serde_json::from_str::<serde_json::Value>("{not-json}").unwrap_err();

    let cases = vec![
        Case {
            error: RociError::Authentication("bad-key".to_string()),
            expected_category: ErrorCategory::Authentication,
            expected_retryable: false,
            expected_recovery: RecoverySuggestion::CheckCredentials,
        },
        Case {
            error: RociError::RateLimited {
                retry_after_ms: Some(1000),
            },
            expected_category: ErrorCategory::RateLimit,
            expected_retryable: true,
            expected_recovery: RecoverySuggestion::RetryWithBackoff,
        },
        Case {
            error: RociError::Timeout(5000),
            expected_category: ErrorCategory::Timeout,
            expected_retryable: true,
            expected_recovery: RecoverySuggestion::IncreaseTimeout,
        },
        Case {
            error: RociError::Configuration("bad-config".to_string()),
            expected_category: ErrorCategory::Configuration,
            expected_retryable: false,
            expected_recovery: RecoverySuggestion::CheckConfiguration,
        },
        Case {
            error: RociError::Network(network_error),
            expected_category: ErrorCategory::Network,
            expected_retryable: true,
            expected_recovery: RecoverySuggestion::RetryWithBackoff,
        },
        Case {
            error: RociError::Serialization(serde_error),
            expected_category: ErrorCategory::Serialization,
            expected_retryable: false,
            expected_recovery: RecoverySuggestion::ContactSupport,
        },
        Case {
            error: RociError::ToolExecution {
                tool_name: "tool-a".to_string(),
                message: "failed".to_string(),
            },
            expected_category: ErrorCategory::ToolExecution,
            expected_retryable: false,
            expected_recovery: RecoverySuggestion::CheckToolImplementation,
        },
        Case {
            error: RociError::api(401, "Unauthorized"),
            expected_category: ErrorCategory::Authentication,
            expected_retryable: false,
            expected_recovery: RecoverySuggestion::CheckCredentials,
        },
        Case {
            error: RociError::api(429, "Rate limited"),
            expected_category: ErrorCategory::RateLimit,
            expected_retryable: true,
            expected_recovery: RecoverySuggestion::RetryWithBackoff,
        },
        Case {
            error: RociError::api(503, "Server unavailable"),
            expected_category: ErrorCategory::Server,
            expected_retryable: true,
            expected_recovery: RecoverySuggestion::RetryWithBackoff,
        },
        Case {
            error: RociError::api(418, "Teapot"),
            expected_category: ErrorCategory::Api,
            expected_retryable: false,
            expected_recovery: RecoverySuggestion::ContactSupport,
        },
        Case {
            error: RociError::Io(io_error),
            expected_category: ErrorCategory::Unknown,
            expected_retryable: false,
            expected_recovery: RecoverySuggestion::ContactSupport,
        },
        Case {
            error: RociError::ModelNotFound("missing".to_string()),
            expected_category: ErrorCategory::Unknown,
            expected_retryable: false,
            expected_recovery: RecoverySuggestion::ContactSupport,
        },
        Case {
            error: RociError::UnsupportedOperation("unsupported".to_string()),
            expected_category: ErrorCategory::Unknown,
            expected_retryable: false,
            expected_recovery: RecoverySuggestion::ContactSupport,
        },
        Case {
            error: RociError::InvalidArgument("bad-arg".to_string()),
            expected_category: ErrorCategory::Unknown,
            expected_retryable: false,
            expected_recovery: RecoverySuggestion::ContactSupport,
        },
        Case {
            error: RociError::Provider {
                provider: "openai".to_string(),
                message: "provider-error".to_string(),
            },
            expected_category: ErrorCategory::Unknown,
            expected_retryable: false,
            expected_recovery: RecoverySuggestion::ContactSupport,
        },
        Case {
            error: RociError::Stream("stream-error".to_string()),
            expected_category: ErrorCategory::Unknown,
            expected_retryable: false,
            expected_recovery: RecoverySuggestion::ContactSupport,
        },
    ];

    for case in cases {
        assert_eq!(case.error.category(), case.expected_category);
        assert_eq!(case.error.is_retryable(), case.expected_retryable);
        assert_eq!(case.error.recovery_suggestion(), case.expected_recovery);
    }
}

#[test]
fn error_api_with_details_sets_detail_fields() {
    let details = ErrorDetails {
        code: Some(ErrorCode::InvalidRequest),
        provider_code: Some("invalid_request_error".to_string()),
        param: Some("messages".to_string()),
        request_id: Some("req-123".to_string()),
    };
    let err = RociError::api_with_details(400, "bad request", details);

    match err {
        RociError::Api {
            status,
            message,
            details: Some(details),
            ..
        } => {
            assert_eq!(status, 400);
            assert_eq!(message, "bad request");
            assert_eq!(details.code, Some(ErrorCode::InvalidRequest));
            assert_eq!(
                details.provider_code.as_deref(),
                Some("invalid_request_error")
            );
            assert_eq!(details.param.as_deref(), Some("messages"));
            assert_eq!(details.request_id.as_deref(), Some("req-123"));
        }
        other => panic!("expected api error with details, got {other:?}"),
    }
}
