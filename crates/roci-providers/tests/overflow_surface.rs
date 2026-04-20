//! Integration test verifying the overflow module is part of the
//! roci-providers public crate surface and usable by downstream consumers.

use roci_core::context::overflow::{OverflowDetectionInput, OverflowDetector, OverflowKind};
use roci_core::error::{ErrorCode, ErrorDetails, RociError};

use roci_providers::overflow::{builtin_overflow_detector, BuiltinOverflowDetector};

#[test]
fn builtin_overflow_detector_is_reachable_from_crate_surface() {
    // Verifies that `roci_providers::overflow::builtin_overflow_detector` compiles
    // and returns a usable detector.
    let detector = builtin_overflow_detector();

    let err = RociError::api_with_details(
        400,
        "This model's maximum context length is 128000 tokens",
        ErrorDetails {
            code: Some(ErrorCode::ContextLengthExceeded),
            provider_code: Some("context_length_exceeded".to_string()),
            param: None,
            request_id: None,
        },
    );
    let input = OverflowDetectionInput::from_error("openai", "gpt-4o", &err);

    let signal = detector.detect(&input).expect("should detect overflow");
    assert_eq!(signal.kind, OverflowKind::InputOverflow);
}

#[test]
fn builtin_overflow_detector_default_trait_is_accessible() {
    let _detector: BuiltinOverflowDetector = Default::default();
}

#[test]
fn provider_code_fast_path_reachable_from_crate_surface() {
    let detector = builtin_overflow_detector();

    let err = RociError::api_with_details(
        400,
        "no overflow keywords in this message",
        ErrorDetails {
            code: None,
            provider_code: Some("context_length_exceeded".to_string()),
            param: None,
            request_id: None,
        },
    );
    let input = OverflowDetectionInput::from_error("openai", "gpt-4o", &err);

    let signal = detector
        .detect(&input)
        .expect("provider_code fast path should fire from crate surface");
    assert_eq!(signal.kind, OverflowKind::InputOverflow);
    assert_eq!(
        signal.provider_code.as_deref(),
        Some("context_length_exceeded")
    );
}
