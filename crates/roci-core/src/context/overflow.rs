//! Provider-agnostic overflow detection and classification contracts.
//!
//! Defines the overflow taxonomy ([`OverflowKind`]), structured detection
//! signal ([`OverflowSignal`]), and the [`OverflowDetector`] trait contract.
//!
//! Concrete classifiers (regex tables, provider-specific matchers) live in
//! `roci-providers`. This module intentionally contains no raw-text matching
//! logic or provider-name branching.

use crate::context::tokens::ContextUsage;
use crate::error::{ErrorCode, RociError};

// ---------------------------------------------------------------------------
// Taxonomy
// ---------------------------------------------------------------------------

/// Provider-agnostic overflow classification.
///
/// Each variant captures a distinct failure mode so recovery logic can pick
/// the right strategy without inspecting raw error strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OverflowKind {
    /// Input context (messages + system prompt) exceeds the model's context
    /// window.
    InputOverflow,
    /// Requested `max_tokens` for the response exceeds provider limits.
    OutputOverflow,
    /// Serialized request body exceeds a provider byte-size limit.
    RequestTooLargeBytes,
    /// The provider silently truncated input or output without returning an
    /// explicit error.
    SilentOverflow,
    /// An overflow occurred but could not be classified into a known category.
    UnknownOverflow,
}

impl OverflowKind {
    /// Default retry hint for this overflow category.
    ///
    /// Detector implementations may override this based on provider-specific
    /// knowledge.
    pub fn default_retry_hint(self) -> OverflowRetryHint {
        match self {
            Self::InputOverflow => OverflowRetryHint::CompactContextFirst,
            Self::OutputOverflow => OverflowRetryHint::ReduceOutputTokensFirst,
            Self::RequestTooLargeBytes => OverflowRetryHint::CompactContextFirst,
            Self::SilentOverflow => OverflowRetryHint::CompactContextFirst,
            Self::UnknownOverflow => OverflowRetryHint::NoAutomaticRecovery,
        }
    }
}

/// Preferred first automatic recovery step for an overflow.
///
/// This is a hint, not a full recovery ladder. The runner policy owns
/// multi-step ordering (e.g. reduce output tokens once, then compact).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OverflowRetryHint {
    /// Reduce `max_tokens` / output budget and retry.
    ReduceOutputTokensFirst,
    /// Compact the conversation context and retry.
    CompactContextFirst,
    /// No safe automatic recovery; surface the error to the caller.
    NoAutomaticRecovery,
}

impl OverflowRetryHint {
    /// Whether this hint represents an automatic recovery strategy.
    pub fn is_automatic(self) -> bool {
        !matches!(self, Self::NoAutomaticRecovery)
    }
}

// ---------------------------------------------------------------------------
// Signal
// ---------------------------------------------------------------------------

/// Structured signal emitted when an overflow is detected.
///
/// Produced by [`OverflowDetector`] implementations. Carries enough context
/// for the agent loop to choose a recovery strategy without re-parsing
/// provider responses.
///
/// **`retry_hint` is the preferred first recovery step**, not a complete
/// recovery ladder. The runner policy owns multi-step recovery ordering
/// (e.g. reduce output tokens once, then compact context).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverflowSignal {
    /// Classified overflow category.
    pub kind: OverflowKind,
    /// Roci-unified error code, if the signal was derived from a typed error.
    pub typed_code: Option<ErrorCode>,
    /// Raw provider error code string (e.g. `"context_length_exceeded"`).
    pub provider_code: Option<String>,
    /// Opaque, provider-owned identifier of the classifier rule that produced
    /// this signal (e.g. `"openai.ctx_len_v2"`, `"anthropic.body_limit"`).
    ///
    /// Optional diagnostic metadata emitted by `roci-providers`. Core attaches
    /// no semantics or stability guarantees to this string — downstream code
    /// must not treat it as a normalized cross-provider identifier.
    pub provider_classifier_id: Option<String>,
    /// Preferred **first** automatic recovery step.
    ///
    /// The runner policy may attempt additional recovery steps beyond this
    /// hint; the signal does not encode the full recovery ladder.
    pub retry_hint: OverflowRetryHint,
}

impl OverflowSignal {
    /// Create a signal with required fields; optional fields default to `None`.
    pub fn new(kind: OverflowKind, retry_hint: OverflowRetryHint) -> Self {
        Self {
            kind,
            typed_code: None,
            provider_code: None,
            provider_classifier_id: None,
            retry_hint,
        }
    }

    /// Set the roci-unified error code.
    #[must_use]
    pub fn with_typed_code(mut self, code: ErrorCode) -> Self {
        self.typed_code = Some(code);
        self
    }

    /// Set the raw provider error code.
    #[must_use]
    pub fn with_provider_code(mut self, code: impl Into<String>) -> Self {
        self.provider_code = Some(code.into());
        self
    }

    /// Attach an opaque provider-owned classifier identifier.
    ///
    /// This is diagnostic metadata with no stability guarantees; core and
    /// downstream consumers must not branch on its value.
    #[must_use]
    pub fn with_provider_classifier_id(mut self, id: impl Into<String>) -> Self {
        self.provider_classifier_id = Some(id.into());
        self
    }

    /// Whether automatic recovery is possible for this signal.
    pub fn is_recoverable(&self) -> bool {
        self.retry_hint.is_automatic()
    }
}

// ---------------------------------------------------------------------------
// Detection input
// ---------------------------------------------------------------------------

/// Input bundle for overflow detection.
///
/// Carries provider name, model ID, an optional error, and an optional
/// context-usage snapshot. Borrows all data to avoid allocation on the
/// detection path.
#[derive(Debug)]
pub struct OverflowDetectionInput<'a> {
    /// Provider identifier (e.g. `"openai"`, `"anthropic"`).
    pub provider: &'a str,
    /// Model identifier (e.g. `"gpt-4o"`, `"claude-sonnet-4-20250514"`).
    pub model_id: &'a str,
    /// The error to classify, when detection is error-driven.
    pub error: Option<&'a RociError>,
    /// Context-window usage snapshot for proactive / silent-overflow detection.
    pub context_usage: Option<&'a ContextUsage>,
}

impl<'a> OverflowDetectionInput<'a> {
    /// Create a detection input for error-driven classification.
    pub fn from_error(provider: &'a str, model_id: &'a str, error: &'a RociError) -> Self {
        Self {
            provider,
            model_id,
            error: Some(error),
            context_usage: None,
        }
    }

    /// Create a detection input for proactive (usage-based) detection.
    pub fn from_usage(provider: &'a str, model_id: &'a str, usage: &'a ContextUsage) -> Self {
        Self {
            provider,
            model_id,
            error: None,
            context_usage: Some(usage),
        }
    }

    /// Attach a context-usage snapshot to an existing input.
    #[must_use]
    pub fn with_context_usage(mut self, usage: &'a ContextUsage) -> Self {
        self.context_usage = Some(usage);
        self
    }
}

// ---------------------------------------------------------------------------
// Detector trait
// ---------------------------------------------------------------------------

/// Contract for provider-specific overflow classifiers.
///
/// Implementations live in `roci-providers`. The core crate defines only
/// the trait and its associated types — no regex tables or provider-name
/// branching belong here.
pub trait OverflowDetector: Send + Sync {
    /// Attempt to classify the input as an overflow.
    ///
    /// Returns `Some(signal)` when the input matches a known overflow
    /// pattern, `None` otherwise.
    fn detect(&self, input: &OverflowDetectionInput<'_>) -> Option<OverflowSignal>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ErrorCode, ErrorDetails};

    // -- OverflowKind -------------------------------------------------------

    #[test]
    fn input_overflow_defaults_to_compact_context() {
        assert_eq!(
            OverflowKind::InputOverflow.default_retry_hint(),
            OverflowRetryHint::CompactContextFirst,
        );
    }

    #[test]
    fn output_overflow_defaults_to_reduce_output_tokens() {
        assert_eq!(
            OverflowKind::OutputOverflow.default_retry_hint(),
            OverflowRetryHint::ReduceOutputTokensFirst,
        );
    }

    #[test]
    fn request_too_large_bytes_defaults_to_compact_context() {
        assert_eq!(
            OverflowKind::RequestTooLargeBytes.default_retry_hint(),
            OverflowRetryHint::CompactContextFirst,
        );
    }

    #[test]
    fn silent_overflow_defaults_to_compact_context() {
        assert_eq!(
            OverflowKind::SilentOverflow.default_retry_hint(),
            OverflowRetryHint::CompactContextFirst,
        );
    }

    #[test]
    fn unknown_overflow_defaults_to_no_recovery() {
        assert_eq!(
            OverflowKind::UnknownOverflow.default_retry_hint(),
            OverflowRetryHint::NoAutomaticRecovery,
        );
    }

    // -- OverflowRetryHint --------------------------------------------------

    #[test]
    fn automatic_hints_report_is_automatic() {
        assert!(OverflowRetryHint::ReduceOutputTokensFirst.is_automatic());
        assert!(OverflowRetryHint::CompactContextFirst.is_automatic());
    }

    #[test]
    fn no_recovery_is_not_automatic() {
        assert!(!OverflowRetryHint::NoAutomaticRecovery.is_automatic());
    }

    // -- OverflowSignal -----------------------------------------------------

    #[test]
    fn signal_new_sets_required_fields_only() {
        let signal = OverflowSignal::new(
            OverflowKind::InputOverflow,
            OverflowRetryHint::CompactContextFirst,
        );
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
        assert_eq!(signal.retry_hint, OverflowRetryHint::CompactContextFirst);
        assert!(signal.typed_code.is_none());
        assert!(signal.provider_code.is_none());
        assert!(signal.provider_classifier_id.is_none());
    }

    #[test]
    fn signal_builder_chain_sets_all_fields() {
        let signal = OverflowSignal::new(
            OverflowKind::InputOverflow,
            OverflowRetryHint::CompactContextFirst,
        )
        .with_typed_code(ErrorCode::ContextLengthExceeded)
        .with_provider_code("context_length_exceeded")
        .with_provider_classifier_id("openai.ctx_len_v2");

        assert_eq!(signal.typed_code, Some(ErrorCode::ContextLengthExceeded));
        assert_eq!(
            signal.provider_code.as_deref(),
            Some("context_length_exceeded"),
        );
        assert_eq!(
            signal.provider_classifier_id.as_deref(),
            Some("openai.ctx_len_v2"),
        );
    }

    #[test]
    fn recoverable_signal_reports_is_recoverable() {
        let signal = OverflowSignal::new(
            OverflowKind::InputOverflow,
            OverflowRetryHint::CompactContextFirst,
        );
        assert!(signal.is_recoverable());
    }

    #[test]
    fn unrecoverable_signal_reports_not_recoverable() {
        let signal = OverflowSignal::new(
            OverflowKind::UnknownOverflow,
            OverflowRetryHint::NoAutomaticRecovery,
        );
        assert!(!signal.is_recoverable());
    }

    // -- OverflowDetectionInput ---------------------------------------------

    #[test]
    fn detection_input_from_error_populates_error() {
        let err = RociError::api_with_details(
            400,
            "context length exceeded",
            ErrorDetails {
                code: Some(ErrorCode::ContextLengthExceeded),
                provider_code: Some("context_length_exceeded".to_string()),
                param: None,
                request_id: None,
            },
        );
        let input = OverflowDetectionInput::from_error("openai", "gpt-4o", &err);

        assert_eq!(input.provider, "openai");
        assert_eq!(input.model_id, "gpt-4o");
        assert!(input.error.is_some());
        assert!(input.context_usage.is_none());
    }

    #[test]
    fn detection_input_from_usage_populates_context() {
        let usage = ContextUsage {
            used_tokens: 120_000,
            context_window: 128_000,
            remaining_tokens: 8_000,
            usage_percent: 93,
        };
        let input =
            OverflowDetectionInput::from_usage("anthropic", "claude-sonnet-4-20250514", &usage);

        assert_eq!(input.provider, "anthropic");
        assert_eq!(input.model_id, "claude-sonnet-4-20250514");
        assert!(input.error.is_none());
        assert_eq!(input.context_usage.unwrap().usage_percent, 93);
    }

    #[test]
    fn detection_input_with_context_usage_attaches_snapshot() {
        let err = RociError::api(400, "too many tokens");
        let usage = ContextUsage {
            used_tokens: 100_000,
            context_window: 128_000,
            remaining_tokens: 28_000,
            usage_percent: 78,
        };
        let input =
            OverflowDetectionInput::from_error("openai", "gpt-4o", &err).with_context_usage(&usage);

        assert!(input.error.is_some());
        assert_eq!(input.context_usage.unwrap().used_tokens, 100_000);
    }

    // -- OverflowDetector trait ---------------------------------------------

    /// No-op detector that never matches — verifies the trait compiles and
    /// is object-safe.
    struct NullDetector;

    impl OverflowDetector for NullDetector {
        fn detect(&self, _input: &OverflowDetectionInput<'_>) -> Option<OverflowSignal> {
            None
        }
    }

    #[test]
    fn null_detector_returns_none() {
        let detector = NullDetector;
        let err = RociError::api(500, "internal error");
        let input = OverflowDetectionInput::from_error("test", "test-model", &err);
        assert!(detector.detect(&input).is_none());
    }

    #[test]
    fn detector_trait_is_object_safe() {
        let detector: Box<dyn OverflowDetector> = Box::new(NullDetector);
        let err = RociError::api(400, "context overflow");
        let input = OverflowDetectionInput::from_error("test", "test-model", &err);
        assert!(detector.detect(&input).is_none());
    }

    /// Stub detector that always returns a fixed signal — verifies a
    /// real implementation can produce [`OverflowSignal`] from the input.
    struct AlwaysInputOverflowDetector;

    impl OverflowDetector for AlwaysInputOverflowDetector {
        fn detect(&self, _input: &OverflowDetectionInput<'_>) -> Option<OverflowSignal> {
            Some(
                OverflowSignal::new(
                    OverflowKind::InputOverflow,
                    OverflowRetryHint::CompactContextFirst,
                )
                .with_typed_code(ErrorCode::ContextLengthExceeded),
            )
        }
    }

    #[test]
    fn stub_detector_returns_expected_signal() {
        let detector = AlwaysInputOverflowDetector;
        let err = RociError::api(400, "context length exceeded");
        let input = OverflowDetectionInput::from_error("openai", "gpt-4o", &err);

        let signal = detector.detect(&input).expect("should detect overflow");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
        assert_eq!(signal.typed_code, Some(ErrorCode::ContextLengthExceeded));
        assert!(signal.is_recoverable());
    }
}
