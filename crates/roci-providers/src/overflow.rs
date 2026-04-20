//! Built-in provider overflow classifiers.
//!
//! Implements [`OverflowDetector`] for providers shipped with roci-providers.
//! All raw text matching and provider-specific parsing lives here — the
//! core crate defines only the trait contract and taxonomy.
//!
//! # Supported providers
//!
//! - **OpenAI family** (Chat, Responses, Codex, Azure, OpenRouter, Together,
//!   and any OpenAI-compatible endpoint that reuses the same error shapes):
//!   typed `ErrorCode::ContextLengthExceeded` fast path, plus regex fallback
//!   for unstructured error messages.
//!
//! - **Anthropic** (including anthropic-compatible transports): text-based
//!   matching today because the transport does not yet preserve
//!   Anthropic-specific structured error details.
//!
//! - **Google Gemini**: text-based matching today because the transport does
//!   not yet preserve Gemini-specific structured error details.

use roci_core::context::overflow::{
    OverflowDetectionInput, OverflowDetector, OverflowKind, OverflowRetryHint, OverflowSignal,
};
use roci_core::error::{ErrorCode, RociError};

// ---------------------------------------------------------------------------
// OpenAI family detector
// ---------------------------------------------------------------------------

/// Overflow detector for the OpenAI family of providers.
///
/// Covers OpenAI Chat, Responses, Codex, Azure OpenAI, OpenRouter, Together,
/// and any OpenAI-compatible endpoint sharing the same error envelope.
///
/// Classification strategy (ordered by reliability):
///
/// 1. **Typed code** — `ErrorDetails::code == ContextLengthExceeded`.
/// 2. **Provider code** — raw `"context_length_exceeded"` string.
/// 3. **Message text** — substring scan for known OpenAI overflow phrases.
pub struct OpenAiOverflowDetector;

impl OpenAiOverflowDetector {
    /// Provider names this detector handles.
    ///
    /// Callers can use this to skip detection for providers that will never
    /// match, though the detector is also safe to call for any provider
    /// (it will simply return `None`).
    pub const PROVIDER_NAMES: &[&str] = &[
        "openai",
        "codex",
        "azure",
        "openrouter",
        "together",
        "openai-compatible",
        "github-copilot",
        "grok",
        "groq",
        "mistral",
        "ollama",
        "lmstudio",
    ];

    /// Check whether this detector applies to the given provider name.
    pub fn handles_provider(provider: &str) -> bool {
        Self::PROVIDER_NAMES.contains(&provider)
    }
}

/// Known overflow-related substrings in OpenAI error messages.
///
/// These are intentionally broad enough to match variant wordings observed
/// across different OpenAI-compatible endpoints, but narrow enough to avoid
/// false positives on unrelated errors.
const OPENAI_INPUT_OVERFLOW_PHRASES: &[&str] = &[
    "context_length_exceeded",
    "maximum context length",
    "context window",
    "too many tokens",
    "token limit",
    "context length exceeded",
    "reduce the length",
    "maximum number of tokens",
];

/// Phrases that indicate an output-side overflow rather than input.
const OPENAI_OUTPUT_OVERFLOW_PHRASES: &[&str] =
    &["max_tokens", "maximum output tokens", "output token limit"];

/// Phrases indicating a byte-size / payload-size limit.
const OPENAI_REQUEST_TOO_LARGE_PHRASES: &[&str] = &[
    "request too large",
    "payload too large",
    "request entity too large",
    "body is too large",
];

impl OverflowDetector for OpenAiOverflowDetector {
    fn detect(&self, input: &OverflowDetectionInput<'_>) -> Option<OverflowSignal> {
        let error = input.error?;

        // ---- Fast path: typed classification --------------------------------
        if let Some(signal) = classify_typed(error) {
            return Some(signal);
        }

        // ---- Slow path: text-based classification ---------------------------
        let message = error_message(error)?;
        classify_text(&message)
    }
}

// ---------------------------------------------------------------------------
// Anthropic detector
// ---------------------------------------------------------------------------

/// Overflow detector for the Anthropic Messages API.
///
/// Anthropic returns structured errors with an `"error"` object containing a
/// `"type"` field. Context-length overflows use type `"invalid_request_error"`
/// with messages about token limits. Since the Anthropic transport currently
/// delegates to the generic `status_to_error` (which does not parse
/// Anthropic-specific structured details), this detector falls back to
/// message-text scanning.
pub struct AnthropicOverflowDetector;

impl AnthropicOverflowDetector {
    /// Provider names this detector handles.
    pub const PROVIDER_NAMES: &[&str] = &["anthropic", "anthropic-compatible"];

    /// Check whether this detector applies to the given provider name.
    pub fn handles_provider(provider: &str) -> bool {
        Self::PROVIDER_NAMES.contains(&provider)
    }
}

/// Known overflow-related substrings in Anthropic error messages.
const ANTHROPIC_INPUT_OVERFLOW_PHRASES: &[&str] = &[
    "prompt is too long",
    "exceeds the model's maximum context",
    "input is too long",
    "too many input tokens",
];

const ANTHROPIC_REQUEST_TOO_LARGE_PHRASES: &[&str] =
    &["request too large", "request entity too large"];

impl OverflowDetector for AnthropicOverflowDetector {
    fn detect(&self, input: &OverflowDetectionInput<'_>) -> Option<OverflowSignal> {
        let error = input.error?;

        // Typed path (if the transport ever populates ErrorDetails)
        if let Some(signal) = classify_typed(error) {
            return Some(signal);
        }

        let message = error_message(error)?;
        let lower = message.to_ascii_lowercase();

        if ANTHROPIC_REQUEST_TOO_LARGE_PHRASES
            .iter()
            .any(|p| lower.contains(p))
        {
            return Some(
                OverflowSignal::new(
                    OverflowKind::RequestTooLargeBytes,
                    OverflowRetryHint::CompactContextFirst,
                )
                .with_provider_classifier_id("anthropic.request_too_large"),
            );
        }

        if ANTHROPIC_INPUT_OVERFLOW_PHRASES
            .iter()
            .any(|p| lower.contains(p))
        {
            return Some(
                OverflowSignal::new(
                    OverflowKind::InputOverflow,
                    OverflowRetryHint::CompactContextFirst,
                )
                .with_provider_classifier_id("anthropic.input_overflow_text"),
            );
        }

        None
    }
}

// ---------------------------------------------------------------------------
// Google detector
// ---------------------------------------------------------------------------

/// Overflow detector for the Google Gemini API.
///
/// Google returns error messages about token limits when the input exceeds
/// the model context window. Like Anthropic, the transport currently uses
/// the generic `status_to_error`, so detection is text-based.
pub struct GoogleOverflowDetector;

impl GoogleOverflowDetector {
    /// Provider names this detector handles.
    pub const PROVIDER_NAMES: &[&str] = &["google"];

    /// Check whether this detector applies to the given provider name.
    pub fn handles_provider(provider: &str) -> bool {
        Self::PROVIDER_NAMES.contains(&provider)
    }
}

/// Known overflow-related substrings in Google Gemini error messages.
const GOOGLE_INPUT_OVERFLOW_PHRASES: &[&str] = &[
    "exceeds the maximum number of tokens",
    "token limit exceeded",
    "input too long",
    "exceeds the model's input token limit",
];

const GOOGLE_REQUEST_TOO_LARGE_PHRASES: &[&str] = &["request payload size exceeds the limit"];

impl OverflowDetector for GoogleOverflowDetector {
    fn detect(&self, input: &OverflowDetectionInput<'_>) -> Option<OverflowSignal> {
        let error = input.error?;

        if let Some(signal) = classify_typed(error) {
            return Some(signal);
        }

        let message = error_message(error)?;
        let lower = message.to_ascii_lowercase();

        if GOOGLE_REQUEST_TOO_LARGE_PHRASES
            .iter()
            .any(|p| lower.contains(p))
        {
            return Some(
                OverflowSignal::new(
                    OverflowKind::RequestTooLargeBytes,
                    OverflowRetryHint::CompactContextFirst,
                )
                .with_provider_classifier_id("google.request_too_large"),
            );
        }

        if GOOGLE_INPUT_OVERFLOW_PHRASES
            .iter()
            .any(|p| lower.contains(p))
        {
            return Some(
                OverflowSignal::new(
                    OverflowKind::InputOverflow,
                    OverflowRetryHint::CompactContextFirst,
                )
                .with_provider_classifier_id("google.input_overflow_text"),
            );
        }

        None
    }
}

// ---------------------------------------------------------------------------
// Composite detector
// ---------------------------------------------------------------------------

/// Composite detector that delegates to the appropriate provider-specific
/// detector based on the `provider` field in [`OverflowDetectionInput`].
///
/// This is the recommended entry point for callers that do not want to
/// manually dispatch by provider name.
pub struct BuiltinOverflowDetector {
    openai: OpenAiOverflowDetector,
    anthropic: AnthropicOverflowDetector,
    google: GoogleOverflowDetector,
}

impl BuiltinOverflowDetector {
    /// Create a composite detector covering all built-in providers.
    pub fn new() -> Self {
        Self {
            openai: OpenAiOverflowDetector,
            anthropic: AnthropicOverflowDetector,
            google: GoogleOverflowDetector,
        }
    }
}

impl Default for BuiltinOverflowDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl OverflowDetector for BuiltinOverflowDetector {
    fn detect(&self, input: &OverflowDetectionInput<'_>) -> Option<OverflowSignal> {
        if OpenAiOverflowDetector::handles_provider(input.provider) {
            return self.openai.detect(input);
        }
        if AnthropicOverflowDetector::handles_provider(input.provider) {
            return self.anthropic.detect(input);
        }
        if GoogleOverflowDetector::handles_provider(input.provider) {
            return self.google.detect(input);
        }
        // Unknown provider — try all detectors as a best-effort fallback.
        self.openai
            .detect(input)
            .or_else(|| self.anthropic.detect(input))
            .or_else(|| self.google.detect(input))
    }
}

/// Create the default built-in overflow detector.
///
/// This is a convenience wrapper around [`BuiltinOverflowDetector::new`].
pub fn builtin_overflow_detector() -> BuiltinOverflowDetector {
    BuiltinOverflowDetector::new()
}

// ---------------------------------------------------------------------------
// Provider wrapper (classify_overflow injection)
// ---------------------------------------------------------------------------

/// Provider wrapper that adds text-based overflow classification to any
/// [`ModelProvider`] using the built-in detector chain.
///
/// Delegates all trait methods to the inner provider. `classify_overflow`
/// first preserves any signal the inner provider already produced, then
/// falls back to the [`BuiltinOverflowDetector`] for provider-specific
/// text matching.
pub struct OverflowClassifyingProvider {
    inner: Box<dyn roci_core::provider::ModelProvider>,
    detector: BuiltinOverflowDetector,
}

impl OverflowClassifyingProvider {
    /// Wrap a provider to gain text-based overflow classification.
    pub fn wrap(
        inner: Box<dyn roci_core::provider::ModelProvider>,
    ) -> Box<dyn roci_core::provider::ModelProvider> {
        Box::new(Self {
            inner,
            detector: BuiltinOverflowDetector::new(),
        })
    }
}

#[async_trait::async_trait]
impl roci_core::provider::ModelProvider for OverflowClassifyingProvider {
    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }

    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn capabilities(&self) -> &roci_core::models::capabilities::ModelCapabilities {
        self.inner.capabilities()
    }

    async fn generate_text(
        &self,
        request: &roci_core::provider::ProviderRequest,
    ) -> Result<roci_core::provider::ProviderResponse, RociError> {
        self.inner.generate_text(request).await
    }

    async fn stream_text(
        &self,
        request: &roci_core::provider::ProviderRequest,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<roci_core::types::TextStreamDelta, RociError>>,
        RociError,
    > {
        self.inner.stream_text(request).await
    }

    fn classify_overflow(&self, error: &RociError) -> Option<OverflowSignal> {
        // Preserve any classification the inner provider already produced.
        if let Some(signal) = self.inner.classify_overflow(error) {
            return Some(signal);
        }
        // Fall back to text-based detection via the built-in detector chain.
        let input = OverflowDetectionInput::from_error(
            self.inner.provider_name(),
            self.inner.model_id(),
            error,
        );
        self.detector.detect(&input)
    }
}

// ---------------------------------------------------------------------------
// Factory wrapper
// ---------------------------------------------------------------------------

/// Factory wrapper that wraps created providers with
/// [`OverflowClassifyingProvider`] at construction time.
///
/// Used by [`register_default_providers`](crate::register_default_providers)
/// to inject text-based overflow classification into all built-in providers
/// without editing every provider implementation.
pub struct OverflowClassifyingFactory {
    inner: std::sync::Arc<dyn roci_core::provider::ProviderFactory>,
}

impl OverflowClassifyingFactory {
    /// Wrap a factory so every provider it creates gains text-based
    /// overflow classification.
    pub fn wrap(
        inner: std::sync::Arc<dyn roci_core::provider::ProviderFactory>,
    ) -> std::sync::Arc<dyn roci_core::provider::ProviderFactory> {
        std::sync::Arc::new(Self { inner })
    }
}

impl roci_core::provider::ProviderFactory for OverflowClassifyingFactory {
    fn provider_keys(&self) -> &[&str] {
        self.inner.provider_keys()
    }

    fn create(
        &self,
        config: &roci_core::config::RociConfig,
        provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn roci_core::provider::ModelProvider>, RociError> {
        let provider = self.inner.create(config, provider_key, model_id)?;
        Ok(OverflowClassifyingProvider::wrap(provider))
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Classify an error via its typed `ErrorDetails`, if present.
fn classify_typed(error: &RociError) -> Option<OverflowSignal> {
    let (_, details) = match error {
        RociError::Api {
            status: _,
            message: _,
            details: Some(d),
            ..
        } => {
            let msg = match error {
                RociError::Api { message, .. } => message.clone(),
                _ => String::new(),
            };
            (msg, d)
        }
        _ => return None,
    };

    match details.code {
        Some(ErrorCode::ContextLengthExceeded) => {
            let mut signal = OverflowSignal::new(
                OverflowKind::InputOverflow,
                OverflowRetryHint::CompactContextFirst,
            )
            .with_typed_code(ErrorCode::ContextLengthExceeded)
            .with_provider_classifier_id("typed.context_length_exceeded");

            if let Some(ref pc) = details.provider_code {
                signal = signal.with_provider_code(pc.clone());
            }

            Some(signal)
        }
        // Provider-code fast path: unified `code` is absent but the raw
        // provider string matches the well-known OpenAI overflow code.
        None if details.provider_code.as_deref() == Some("context_length_exceeded") => Some(
            OverflowSignal::new(
                OverflowKind::InputOverflow,
                OverflowRetryHint::CompactContextFirst,
            )
            .with_provider_code("context_length_exceeded".to_string())
            .with_provider_classifier_id("provider_code.context_length_exceeded"),
        ),
        _ => None,
    }
}

/// Extract the human-readable message from a `RociError`, if available.
fn error_message(error: &RociError) -> Option<String> {
    match error {
        RociError::Api { message, .. } => Some(message.clone()),
        _ => Some(error.to_string()),
    }
}

/// Classify overflow from raw message text (OpenAI family).
fn classify_text(message: &str) -> Option<OverflowSignal> {
    let lower = message.to_ascii_lowercase();

    // Check request-too-large first (more specific).
    if OPENAI_REQUEST_TOO_LARGE_PHRASES
        .iter()
        .any(|p| lower.contains(p))
    {
        return Some(
            OverflowSignal::new(
                OverflowKind::RequestTooLargeBytes,
                OverflowRetryHint::CompactContextFirst,
            )
            .with_provider_classifier_id("openai.request_too_large_text"),
        );
    }

    // Check output overflow before input (more specific).
    if OPENAI_OUTPUT_OVERFLOW_PHRASES
        .iter()
        .any(|p| lower.contains(p))
    {
        return Some(
            OverflowSignal::new(
                OverflowKind::OutputOverflow,
                OverflowRetryHint::ReduceOutputTokensFirst,
            )
            .with_provider_classifier_id("openai.output_overflow_text"),
        );
    }

    // Input overflow (broadest match).
    if OPENAI_INPUT_OVERFLOW_PHRASES
        .iter()
        .any(|p| lower.contains(p))
    {
        return Some(
            OverflowSignal::new(
                OverflowKind::InputOverflow,
                OverflowRetryHint::CompactContextFirst,
            )
            .with_provider_classifier_id("openai.input_overflow_text"),
        );
    }

    None
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use roci_core::error::ErrorDetails;

    // -- Helpers ------------------------------------------------------------

    fn make_api_error(status: u16, message: &str) -> RociError {
        RociError::api(status, message)
    }

    fn make_typed_error(code: ErrorCode, provider_code: &str, message: &str) -> RociError {
        RociError::api_with_details(
            400,
            message,
            ErrorDetails {
                code: Some(code),
                provider_code: Some(provider_code.to_string()),
                param: None,
                request_id: None,
            },
        )
    }

    fn input_for_error<'a>(
        provider: &'a str,
        model: &'a str,
        error: &'a RociError,
    ) -> OverflowDetectionInput<'a> {
        OverflowDetectionInput::from_error(provider, model, error)
    }

    // ======================================================================
    // OpenAI typed code path
    // ======================================================================

    #[test]
    fn openai_typed_context_length_exceeded_detects_input_overflow() {
        let detector = OpenAiOverflowDetector;
        let err = make_typed_error(
            ErrorCode::ContextLengthExceeded,
            "context_length_exceeded",
            "This model's maximum context length is 128000 tokens",
        );
        let input = input_for_error("openai", "gpt-4o", &err);

        let signal = detector.detect(&input).expect("should detect overflow");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
        assert_eq!(signal.typed_code, Some(ErrorCode::ContextLengthExceeded));
        assert_eq!(
            signal.provider_code.as_deref(),
            Some("context_length_exceeded")
        );
        assert_eq!(signal.retry_hint, OverflowRetryHint::CompactContextFirst);
        assert!(signal.is_recoverable());
    }

    #[test]
    fn openai_typed_unknown_code_does_not_detect() {
        let detector = OpenAiOverflowDetector;
        let err = make_typed_error(
            ErrorCode::InvalidRequest,
            "invalid_request_error",
            "invalid temperature value",
        );
        let input = input_for_error("openai", "gpt-4o", &err);

        assert!(detector.detect(&input).is_none());
    }

    // ======================================================================
    // OpenAI text fallback path
    // ======================================================================

    #[test]
    fn openai_text_maximum_context_length_detects_input_overflow() {
        let detector = OpenAiOverflowDetector;
        let err = make_api_error(
            400,
            "This model's maximum context length is 16385 tokens. \
             However, your messages resulted in 20000 tokens.",
        );
        let input = input_for_error("openai", "gpt-3.5-turbo", &err);

        let signal = detector.detect(&input).expect("should detect overflow");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
        assert_eq!(
            signal.provider_classifier_id.as_deref(),
            Some("openai.input_overflow_text")
        );
    }

    #[test]
    fn openai_text_too_many_tokens_detects_input_overflow() {
        let detector = OpenAiOverflowDetector;
        let err = make_api_error(400, "Too many tokens in the input");
        let input = input_for_error("openai", "gpt-4o", &err);

        let signal = detector.detect(&input).expect("should detect overflow");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
    }

    #[test]
    fn openai_text_reduce_the_length_detects_input_overflow() {
        let detector = OpenAiOverflowDetector;
        let err = make_api_error(
            400,
            "Please reduce the length of the messages or completion.",
        );
        let input = input_for_error("openai", "gpt-4", &err);

        let signal = detector.detect(&input).expect("should detect overflow");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
    }

    #[test]
    fn openai_text_max_tokens_detects_output_overflow() {
        let detector = OpenAiOverflowDetector;
        let err = make_api_error(
            400,
            "max_tokens is too large: 128000. This model supports at most 4096 output tokens.",
        );
        let input = input_for_error("openai", "gpt-4", &err);

        let signal = detector.detect(&input).expect("should detect overflow");
        assert_eq!(signal.kind, OverflowKind::OutputOverflow);
        assert_eq!(
            signal.retry_hint,
            OverflowRetryHint::ReduceOutputTokensFirst
        );
    }

    #[test]
    fn openai_text_request_too_large_detects_payload_overflow() {
        let detector = OpenAiOverflowDetector;
        let err = make_api_error(413, "Request too large for model");
        let input = input_for_error("openai", "gpt-4o", &err);

        let signal = detector.detect(&input).expect("should detect overflow");
        assert_eq!(signal.kind, OverflowKind::RequestTooLargeBytes);
    }

    #[test]
    fn openai_text_unrelated_error_returns_none() {
        let detector = OpenAiOverflowDetector;
        let err = make_api_error(400, "Invalid value for temperature");
        let input = input_for_error("openai", "gpt-4o", &err);

        assert!(detector.detect(&input).is_none());
    }

    #[test]
    fn openai_text_context_window_detects_input_overflow() {
        let detector = OpenAiOverflowDetector;
        let err = make_api_error(400, "Input exceeds the context window of this model.");
        let input = input_for_error("openai", "gpt-4o-mini", &err);

        let signal = detector.detect(&input).expect("should detect overflow");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
    }

    // ======================================================================
    // OpenAI-compatible providers
    // ======================================================================

    #[test]
    fn codex_typed_context_overflow_detected() {
        let detector = OpenAiOverflowDetector;
        let err = make_typed_error(
            ErrorCode::ContextLengthExceeded,
            "context_length_exceeded",
            "context length exceeded",
        );
        let input = input_for_error("codex", "codex-mini-latest", &err);

        let signal = detector.detect(&input).expect("should detect");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
    }

    #[test]
    fn azure_text_overflow_detected() {
        let detector = OpenAiOverflowDetector;
        let err = make_api_error(400, "This model's maximum context length is 128000 tokens");
        let input = input_for_error("azure", "gpt-4o", &err);

        let signal = detector.detect(&input).expect("should detect");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
    }

    #[test]
    fn openrouter_text_overflow_detected() {
        let detector = OpenAiOverflowDetector;
        let err = make_api_error(400, "Token limit exceeded for this model");
        let input = input_for_error("openrouter", "meta-llama/llama-3", &err);

        let signal = detector.detect(&input).expect("should detect");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
    }

    // ======================================================================
    // Handles-provider guard
    // ======================================================================

    #[test]
    fn handles_provider_returns_true_for_openai_family() {
        assert!(OpenAiOverflowDetector::handles_provider("openai"));
        assert!(OpenAiOverflowDetector::handles_provider("codex"));
        assert!(OpenAiOverflowDetector::handles_provider("azure"));
        assert!(OpenAiOverflowDetector::handles_provider("openrouter"));
        assert!(OpenAiOverflowDetector::handles_provider("github-copilot"));
    }

    #[test]
    fn handles_provider_returns_false_for_unknown() {
        assert!(!OpenAiOverflowDetector::handles_provider("anthropic"));
        assert!(!OpenAiOverflowDetector::handles_provider("google"));
        assert!(!OpenAiOverflowDetector::handles_provider("custom"));
    }

    // ======================================================================
    // Non-error input returns None
    // ======================================================================

    #[test]
    fn no_error_returns_none() {
        let detector = OpenAiOverflowDetector;
        let usage = roci_core::context::tokens::ContextUsage {
            used_tokens: 100_000,
            context_window: 128_000,
            remaining_tokens: 28_000,
            usage_percent: 78,
        };
        let input = OverflowDetectionInput::from_usage("openai", "gpt-4o", &usage);

        assert!(detector.detect(&input).is_none());
    }

    // ======================================================================
    // Anthropic detector
    // ======================================================================

    #[test]
    fn anthropic_typed_context_length_exceeded_detects() {
        let detector = AnthropicOverflowDetector;
        let err = make_typed_error(
            ErrorCode::ContextLengthExceeded,
            "context_length_exceeded",
            "prompt is too long",
        );
        let input = input_for_error("anthropic", "claude-sonnet-4-20250514", &err);

        let signal = detector.detect(&input).expect("should detect");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
        assert_eq!(signal.typed_code, Some(ErrorCode::ContextLengthExceeded));
    }

    #[test]
    fn anthropic_text_prompt_too_long_detects() {
        let detector = AnthropicOverflowDetector;
        let err = make_api_error(400, "prompt is too long: 250000 tokens > 200000 maximum");
        let input = input_for_error("anthropic", "claude-sonnet-4-20250514", &err);

        let signal = detector.detect(&input).expect("should detect");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
        assert_eq!(
            signal.provider_classifier_id.as_deref(),
            Some("anthropic.input_overflow_text")
        );
    }

    #[test]
    fn anthropic_text_request_too_large_detects() {
        let detector = AnthropicOverflowDetector;
        let err = make_api_error(413, "request too large");
        let input = input_for_error("anthropic", "claude-sonnet-4-20250514", &err);

        let signal = detector.detect(&input).expect("should detect");
        assert_eq!(signal.kind, OverflowKind::RequestTooLargeBytes);
    }

    #[test]
    fn anthropic_unrelated_error_returns_none() {
        let detector = AnthropicOverflowDetector;
        let err = make_api_error(400, "invalid api key format");
        let input = input_for_error("anthropic", "claude-sonnet-4-20250514", &err);

        assert!(detector.detect(&input).is_none());
    }

    #[test]
    fn anthropic_handles_provider_checks() {
        assert!(AnthropicOverflowDetector::handles_provider("anthropic"));
        assert!(AnthropicOverflowDetector::handles_provider(
            "anthropic-compatible"
        ));
        assert!(!AnthropicOverflowDetector::handles_provider("openai"));
    }

    // ======================================================================
    // Google detector
    // ======================================================================

    #[test]
    fn google_typed_context_length_exceeded_detects() {
        let detector = GoogleOverflowDetector;
        let err = make_typed_error(
            ErrorCode::ContextLengthExceeded,
            "context_length_exceeded",
            "exceeds the maximum number of tokens",
        );
        let input = input_for_error("google", "gemini-2.5-pro", &err);

        let signal = detector.detect(&input).expect("should detect");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
    }

    #[test]
    fn google_text_exceeds_max_tokens_detects() {
        let detector = GoogleOverflowDetector;
        let err = make_api_error(
            400,
            "Request exceeds the maximum number of tokens allowed for this model",
        );
        let input = input_for_error("google", "gemini-2.5-flash", &err);

        let signal = detector.detect(&input).expect("should detect");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
        assert_eq!(
            signal.provider_classifier_id.as_deref(),
            Some("google.input_overflow_text")
        );
    }

    #[test]
    fn google_text_payload_too_large_detects() {
        let detector = GoogleOverflowDetector;
        let err = make_api_error(413, "Request payload size exceeds the limit");
        let input = input_for_error("google", "gemini-2.5-pro", &err);

        let signal = detector.detect(&input).expect("should detect");
        assert_eq!(signal.kind, OverflowKind::RequestTooLargeBytes);
    }

    #[test]
    fn google_unrelated_error_returns_none() {
        let detector = GoogleOverflowDetector;
        let err = make_api_error(400, "API key not valid");
        let input = input_for_error("google", "gemini-2.5-flash", &err);

        assert!(detector.detect(&input).is_none());
    }

    #[test]
    fn google_handles_provider_checks() {
        assert!(GoogleOverflowDetector::handles_provider("google"));
        assert!(!GoogleOverflowDetector::handles_provider("openai"));
    }

    // ======================================================================
    // Composite detector
    // ======================================================================

    #[test]
    fn composite_routes_openai_to_openai_detector() {
        let detector = BuiltinOverflowDetector::new();
        let err = make_typed_error(
            ErrorCode::ContextLengthExceeded,
            "context_length_exceeded",
            "context length exceeded",
        );
        let input = input_for_error("openai", "gpt-4o", &err);

        let signal = detector.detect(&input).expect("should detect");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
    }

    #[test]
    fn composite_routes_anthropic_to_anthropic_detector() {
        let detector = BuiltinOverflowDetector::new();
        let err = make_api_error(400, "prompt is too long: 300000 tokens");
        let input = input_for_error("anthropic", "claude-sonnet-4-20250514", &err);

        let signal = detector.detect(&input).expect("should detect");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
    }

    #[test]
    fn composite_routes_google_to_google_detector() {
        let detector = BuiltinOverflowDetector::new();
        let err = make_api_error(400, "Request exceeds the maximum number of tokens allowed");
        let input = input_for_error("google", "gemini-2.5-pro", &err);

        let signal = detector.detect(&input).expect("should detect");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
    }

    #[test]
    fn composite_unknown_provider_tries_all_detectors() {
        let detector = BuiltinOverflowDetector::new();
        let err = make_api_error(400, "This model's maximum context length is 8192 tokens");
        let input = input_for_error("custom-provider", "custom-model", &err);

        let signal = detector.detect(&input).expect("should detect via fallback");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
    }

    #[test]
    fn composite_unknown_provider_unrelated_error_returns_none() {
        let detector = BuiltinOverflowDetector::new();
        let err = make_api_error(500, "internal server error");
        let input = input_for_error("custom-provider", "custom-model", &err);

        assert!(detector.detect(&input).is_none());
    }

    #[test]
    fn composite_default_is_equivalent_to_new() {
        let d1 = BuiltinOverflowDetector::new();
        let d2 = BuiltinOverflowDetector::default();
        let err = make_typed_error(
            ErrorCode::ContextLengthExceeded,
            "context_length_exceeded",
            "overflow",
        );
        let input = input_for_error("openai", "gpt-4o", &err);

        assert_eq!(d1.detect(&input), d2.detect(&input));
    }

    #[test]
    fn builtin_overflow_detector_fn_creates_composite() {
        let detector = builtin_overflow_detector();
        let err = make_typed_error(
            ErrorCode::ContextLengthExceeded,
            "context_length_exceeded",
            "overflow",
        );
        let input = input_for_error("openai", "gpt-4o", &err);

        assert!(detector.detect(&input).is_some());
    }

    // ======================================================================
    // Case insensitivity
    // ======================================================================

    #[test]
    fn text_matching_is_case_insensitive() {
        let detector = OpenAiOverflowDetector;
        let err = make_api_error(400, "MAXIMUM CONTEXT LENGTH is 128000 tokens");
        let input = input_for_error("openai", "gpt-4o", &err);

        assert!(detector.detect(&input).is_some());
    }

    // ======================================================================
    // Provider-code fast path (code=None, provider_code present)
    // ======================================================================

    #[test]
    fn provider_code_fast_path_detects_when_unified_code_is_none() {
        let detector = OpenAiOverflowDetector;
        let err = RociError::api_with_details(
            400,
            "unstructured message without overflow phrases",
            ErrorDetails {
                code: None,
                provider_code: Some("context_length_exceeded".to_string()),
                param: None,
                request_id: None,
            },
        );
        let input = input_for_error("openai", "gpt-4o", &err);

        let signal = detector
            .detect(&input)
            .expect("provider_code fast path should detect");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
        assert_eq!(signal.retry_hint, OverflowRetryHint::CompactContextFirst);
        assert_eq!(
            signal.provider_code.as_deref(),
            Some("context_length_exceeded")
        );
        assert_eq!(
            signal.provider_classifier_id.as_deref(),
            Some("provider_code.context_length_exceeded"),
        );
        // Typed code should be None since the unified code was absent.
        assert_eq!(signal.typed_code, None);
    }

    #[test]
    fn provider_code_fast_path_ignored_for_non_overflow_provider_code() {
        let detector = OpenAiOverflowDetector;
        let err = RociError::api_with_details(
            400,
            "something went wrong",
            ErrorDetails {
                code: None,
                provider_code: Some("invalid_api_key".to_string()),
                param: None,
                request_id: None,
            },
        );
        let input = input_for_error("openai", "gpt-4o", &err);

        assert!(detector.detect(&input).is_none());
    }

    #[test]
    fn provider_code_fast_path_works_via_composite_detector() {
        let detector = BuiltinOverflowDetector::new();
        let err = RociError::api_with_details(
            400,
            "no overflow phrases here",
            ErrorDetails {
                code: None,
                provider_code: Some("context_length_exceeded".to_string()),
                param: None,
                request_id: None,
            },
        );
        let input = input_for_error("openai", "gpt-4o", &err);

        let signal = detector
            .detect(&input)
            .expect("composite should find via provider_code");
        assert_eq!(signal.kind, OverflowKind::InputOverflow);
    }
}
