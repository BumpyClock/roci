//! Token estimation utilities for context window management.
//!
//! Provides fast, provider-agnostic heuristic token counts based on character
//! length. These estimates are intentionally cheap (no tokenizer dependency)
//! and accurate enough for budget/overflow decisions. Provider-specific exact
//! tokenizers will live in `roci-providers` behind feature-gated integrations.
//!
//! # Types
//!
//! - [`TokenCount`] — a token count with accuracy and provenance metadata
//! - [`CountAccuracy`] — whether the count is exact or estimated
//! - [`TokenCountSource`] — where the count came from (heuristic, tokenizer, provider)
//! - [`TokenCounter`] — trait for pluggable token counting strategies
//! - [`HeuristicTokenCounter`] — default ~4 chars/token implementation
//!
//! # Backward-compatible free functions
//!
//! [`estimate_text_tokens`], [`estimate_message_tokens`], and
//! [`estimate_context_usage`] are retained for existing call sites. They
//! delegate to [`HeuristicTokenCounter`] internally.

use crate::types::{ContentPart, ModelMessage, Role};

// ---------------------------------------------------------------------------
// Metadata types
// ---------------------------------------------------------------------------

/// How accurate a token count is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CountAccuracy {
    /// Exact count from a model-specific tokenizer or provider usage report.
    Exact,
    /// Heuristic estimate (e.g., chars / 4).
    Estimated,
}

/// Where a token count originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenCountSource {
    /// Cheap character-ratio heuristic.
    Heuristic,
    /// Model-specific tokenizer (e.g., tiktoken, sentencepiece).
    ExactTokenizer,
    /// Provider usage metadata returned alongside a completion.
    ProviderUsage,
}

impl TokenCountSource {
    /// Lower rank = less reliable. Used when merging heterogeneous counts.
    fn reliability_rank(self) -> u8 {
        match self {
            Self::Heuristic => 0,
            Self::ExactTokenizer => 1,
            Self::ProviderUsage => 2,
        }
    }
}

// ---------------------------------------------------------------------------
// TokenCount value type
// ---------------------------------------------------------------------------

/// A token count annotated with accuracy and source metadata.
///
/// Supports `+` / `+=` for accumulation and `Iterator::sum()`. When counts
/// from different sources are combined, accuracy degrades to [`CountAccuracy::Estimated`]
/// if any component is estimated, and source falls back to the least reliable
/// contributor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenCount {
    pub tokens: usize,
    pub accuracy: CountAccuracy,
    pub source: TokenCountSource,
}

impl TokenCount {
    /// Create an estimated count from the heuristic counter.
    pub fn heuristic(tokens: usize) -> Self {
        Self {
            tokens,
            accuracy: CountAccuracy::Estimated,
            source: TokenCountSource::Heuristic,
        }
    }

    /// Create an exact count from the given source.
    pub fn exact(tokens: usize, source: TokenCountSource) -> Self {
        Self {
            tokens,
            accuracy: CountAccuracy::Exact,
            source,
        }
    }

    /// The additive identity — zero tokens, exact accuracy.
    pub fn zero() -> Self {
        Self {
            tokens: 0,
            accuracy: CountAccuracy::Exact,
            source: TokenCountSource::Heuristic,
        }
    }
}

impl std::ops::Add for TokenCount {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        let accuracy = match (self.accuracy, rhs.accuracy) {
            (CountAccuracy::Exact, CountAccuracy::Exact) => CountAccuracy::Exact,
            _ => CountAccuracy::Estimated,
        };
        // For zero-token operands, preserve the other's source.
        // Otherwise, keep the least reliable (lowest rank) source.
        let source = if self.tokens == 0 {
            rhs.source
        } else if rhs.tokens == 0 || self.source.reliability_rank() <= rhs.source.reliability_rank()
        {
            self.source
        } else {
            rhs.source
        };
        Self {
            tokens: self.tokens + rhs.tokens,
            accuracy,
            source,
        }
    }
}

impl std::ops::AddAssign for TokenCount {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl std::iter::Sum for TokenCount {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::zero(), |acc, x| acc + x)
    }
}

// ---------------------------------------------------------------------------
// TokenCounter trait
// ---------------------------------------------------------------------------

/// Strategy for counting tokens in text and messages.
///
/// Implement [`count_text`](Self::count_text) to plug in a new tokenizer.
/// Default implementations of [`count_message`](Self::count_message) and
/// [`count_messages`](Self::count_messages) decompose messages into text
/// parts, add heuristic framing overhead, and delegate to `count_text`.
///
/// Exact tokenizers that understand message framing natively should override
/// `count_message` as well.
pub trait TokenCounter {
    /// Count tokens in a plain text string.
    fn count_text(&self, text: &str) -> TokenCount;

    /// Count tokens for a single [`ModelMessage`].
    ///
    /// The default implementation decomposes content parts, counts each via
    /// [`count_text`](Self::count_text), and adds heuristic overhead for
    /// message framing (3 tokens for start/end markers) plus the role string,
    /// and structured parts (8 tokens each for images, tool calls, and tool
    /// results). The message role string is included in the count.
    fn count_message(&self, message: &ModelMessage) -> TokenCount {
        let mut count = TokenCount::heuristic(3); // per-message framing (start/end markers)
        let role_str = match message.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        count += self.count_text(role_str);
        for part in &message.content {
            count += match part {
                ContentPart::Text { text } => self.count_text(text),
                ContentPart::Image(image) => {
                    self.count_text(&image.data)
                        + self.count_text(&image.mime_type)
                        + TokenCount::heuristic(8)
                }
                ContentPart::ToolCall(tc) => {
                    let args = tc.arguments.to_string();
                    let mut c = self.count_text(&tc.id)
                        + self.count_text(&tc.name)
                        + self.count_text(&args)
                        + TokenCount::heuristic(8);
                    if let Some(recipient) = &tc.recipient {
                        c += self.count_text(recipient);
                    }
                    c
                }
                ContentPart::ToolResult(result) => {
                    let payload = result.result.to_string();
                    self.count_text(&result.tool_call_id)
                        + self.count_text(&payload)
                        + self.count_text(&result.is_error.to_string())
                        + TokenCount::heuristic(8)
                }
                ContentPart::Thinking(thinking) => {
                    self.count_text(&thinking.thinking) + self.count_text(&thinking.signature)
                }
                ContentPart::RedactedThinking(thinking) => {
                    self.count_text(&thinking.data) + self.count_text(&thinking.signature)
                }
            };
        }
        if let Some(name) = &message.name {
            count += self.count_text(name);
        }
        count
    }

    /// Count tokens for a slice of messages.
    ///
    /// Default implementation sums individual [`count_message`](Self::count_message) results.
    fn count_messages(&self, messages: &[ModelMessage]) -> TokenCount {
        messages.iter().map(|m| self.count_message(m)).sum()
    }
}

// ---------------------------------------------------------------------------
// HeuristicTokenCounter
// ---------------------------------------------------------------------------

/// Default token counter using the ~4 characters/token heuristic.
///
/// This is the cheapest counting strategy — no tokenizer dependency, no
/// allocations beyond those inherent in content-part serialization. Accuracy
/// is sufficient for budget/overflow decisions; exact counts come from
/// provider-specific integrations in `roci-providers`.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeuristicTokenCounter;

impl TokenCounter for HeuristicTokenCounter {
    fn count_text(&self, text: &str) -> TokenCount {
        if text.is_empty() {
            return TokenCount::zero();
        }
        TokenCount::heuristic(text.chars().count().div_ceil(4))
    }
}

// ---------------------------------------------------------------------------
// Backward-compatible free functions
// ---------------------------------------------------------------------------

/// Snapshot of how much of the context window is consumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextUsage {
    pub used_tokens: usize,
    pub context_window: usize,
    pub remaining_tokens: usize,
    pub usage_percent: u8,
}

/// Estimate token count for a plain text string.
///
/// Uses the common ~4 chars/token heuristic. Returns 0 for empty input.
/// Backed by [`HeuristicTokenCounter`].
pub fn estimate_text_tokens(text: &str) -> usize {
    HeuristicTokenCounter.count_text(text).tokens
}

/// Estimate token count for a single [`ModelMessage`].
///
/// Accounts for all content parts (text, images, tool calls/results, thinking)
/// plus per-message overhead. Backed by [`HeuristicTokenCounter`].
pub fn estimate_message_tokens(message: &ModelMessage) -> usize {
    HeuristicTokenCounter.count_message(message).tokens
}

/// Compute [`ContextUsage`] for a message slice against a context window size.
pub fn estimate_context_usage(messages: &[ModelMessage], context_window: usize) -> ContextUsage {
    let used_tokens: usize = HeuristicTokenCounter.count_messages(messages).tokens;
    let remaining_tokens = context_window.saturating_sub(used_tokens);
    let usage_percent = if context_window == 0 {
        100
    } else {
        ((used_tokens.saturating_mul(100)) / context_window).min(100) as u8
    };

    ContextUsage {
        used_tokens,
        context_window,
        remaining_tokens,
        usage_percent,
    }
}

// ---------------------------------------------------------------------------
// ContextUsageSnapshot — confidence/source metadata
// ---------------------------------------------------------------------------

/// Overall confidence in a [`ContextUsageSnapshot`].
///
/// Derived automatically from the data sources that contributed to the
/// snapshot. Callers can use this to decide how aggressively to trust
/// the numbers (e.g. a `Low` snapshot might trigger a conservative
/// compaction threshold).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnapshotConfidence {
    /// All token counts are provider-reported exact values.
    High,
    /// Anchor is exact (provider-reported) but includes estimated tail
    /// tokens for content added after the last API response.
    Medium,
    /// Entire snapshot is heuristic-estimated (no provider data).
    Low,
}

/// How the snapshot's anchor data was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnapshotSource {
    /// Provider-reported usage data (`prompt_tokens` / `completion_tokens`).
    ProviderReported,
    /// Full heuristic estimate (no provider data available).
    FullEstimate,
}

// ---------------------------------------------------------------------------
// ContextUsageSnapshot
// ---------------------------------------------------------------------------

/// Rich, confidence-annotated snapshot of context window consumption.
///
/// Combines exact provider-reported usage (the "anchor") with estimated
/// tail tokens for content added after the last provider response. This
/// gives the most accurate view possible at any point in the agent loop
/// without requiring a round-trip to re-count everything through a
/// provider tokenizer.
///
/// # Construction
///
/// Use the named constructors to create snapshots from different data
/// sources:
///
/// - [`from_provider`] — exact provider data only (no tail).
/// - [`from_provider_with_tail`] — exact anchor plus estimated tail.
/// - [`from_estimate`] — fully estimated, no provider data.
///
/// # Relationship to [`ContextUsage`]
///
/// [`ContextUsage`] is the backward-compatible flat snapshot used by
/// existing call sites. [`ContextUsageSnapshot`] is the richer
/// replacement that preserves provenance and confidence metadata.
/// Use [`to_context_usage`](Self::to_context_usage) to convert when
/// interfacing with code that expects the older type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextUsageSnapshot {
    /// Context window size in tokens.
    pub context_window: usize,

    /// Exact provider-reported input (prompt) tokens.
    /// `None` when no provider data is available.
    pub provider_prompt_tokens: Option<usize>,

    /// Exact provider-reported output (completion) tokens.
    /// `None` when no provider data is available.
    pub provider_completion_tokens: Option<usize>,

    /// Estimated tokens for content not covered by provider usage.
    ///
    /// This is the "tail" — messages or content added after the last API
    /// call whose tokens have not been counted by the provider. Zero when
    /// provider usage covers the entire context.
    pub estimated_tail: TokenCount,

    /// Total tokens consumed (anchor + tail).
    pub total_used: TokenCount,

    /// Remaining tokens in the context window.
    pub remaining_tokens: usize,

    /// Usage as a percentage of the context window (0–100, clamped).
    pub usage_percent: u8,

    /// Overall confidence in this snapshot's accuracy.
    pub confidence: SnapshotConfidence,

    /// How the anchor data was obtained.
    pub source: SnapshotSource,
}

impl ContextUsageSnapshot {
    /// Create a snapshot from exact provider-reported usage only.
    ///
    /// Use when the provider response covers the entire context and no
    /// new content has been added since.
    pub fn from_provider(
        context_window: usize,
        prompt_tokens: usize,
        completion_tokens: usize,
    ) -> Self {
        let total_tokens = prompt_tokens + completion_tokens;
        let total_used = TokenCount::exact(total_tokens, TokenCountSource::ProviderUsage);

        Self {
            context_window,
            provider_prompt_tokens: Some(prompt_tokens),
            provider_completion_tokens: Some(completion_tokens),
            estimated_tail: TokenCount::zero(),
            total_used,
            remaining_tokens: context_window.saturating_sub(total_tokens),
            usage_percent: usage_pct(total_tokens, context_window),
            confidence: SnapshotConfidence::High,
            source: SnapshotSource::ProviderReported,
        }
    }

    /// Create a snapshot from exact provider usage plus estimated tail.
    ///
    /// Use when messages have been added after the last API response
    /// (e.g. user input, tool results) whose tokens are only estimated.
    pub fn from_provider_with_tail(
        context_window: usize,
        prompt_tokens: usize,
        completion_tokens: usize,
        tail: TokenCount,
    ) -> Self {
        let anchor = TokenCount::exact(
            prompt_tokens + completion_tokens,
            TokenCountSource::ProviderUsage,
        );
        let total_used = anchor + tail;
        let remaining = context_window.saturating_sub(total_used.tokens);

        // If tail is zero, the snapshot is fully exact.
        let confidence = if tail.tokens == 0 {
            SnapshotConfidence::High
        } else {
            SnapshotConfidence::Medium
        };

        Self {
            context_window,
            provider_prompt_tokens: Some(prompt_tokens),
            provider_completion_tokens: Some(completion_tokens),
            estimated_tail: tail,
            total_used,
            remaining_tokens: remaining,
            usage_percent: usage_pct(total_used.tokens, context_window),
            confidence,
            source: SnapshotSource::ProviderReported,
        }
    }

    /// Create a fully estimated snapshot with no provider data.
    ///
    /// Use when no provider usage metadata is available (e.g. before the
    /// first API call, or with providers that don't report token counts).
    pub fn from_estimate(context_window: usize, estimated: TokenCount) -> Self {
        let remaining = context_window.saturating_sub(estimated.tokens);
        Self {
            context_window,
            provider_prompt_tokens: None,
            provider_completion_tokens: None,
            estimated_tail: estimated,
            total_used: estimated,
            remaining_tokens: remaining,
            usage_percent: usage_pct(estimated.tokens, context_window),
            confidence: SnapshotConfidence::Low,
            source: SnapshotSource::FullEstimate,
        }
    }

    /// Convert to the backward-compatible [`ContextUsage`] type.
    ///
    /// Discards confidence/source metadata. Use when interfacing with
    /// code that expects the flat [`ContextUsage`] struct.
    pub fn to_context_usage(&self) -> ContextUsage {
        ContextUsage {
            used_tokens: self.total_used.tokens,
            context_window: self.context_window,
            remaining_tokens: self.remaining_tokens,
            usage_percent: self.usage_percent,
        }
    }

    /// Whether the snapshot is based on exact provider data (possibly
    /// with estimated tail) as opposed to a full estimate.
    pub fn has_provider_anchor(&self) -> bool {
        self.provider_prompt_tokens.is_some()
    }

    /// The fraction of total usage that is estimated (0.0–1.0).
    ///
    /// Returns 0.0 when everything is exact, 1.0 when fully estimated.
    pub fn estimated_fraction(&self) -> f64 {
        if self.total_used.tokens == 0 {
            return 0.0;
        }
        self.estimated_tail.tokens as f64 / self.total_used.tokens as f64
    }
}

/// Compute usage percentage clamped to 0–100.
fn usage_pct(used: usize, window: usize) -> u8 {
    if window == 0 {
        return 100;
    }
    (used.saturating_mul(100) / window).min(100) as u8
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ModelMessage;

    // -- TokenCount construction --

    #[test]
    fn token_count_heuristic_has_estimated_accuracy() {
        let tc = TokenCount::heuristic(42);
        assert_eq!(tc.tokens, 42);
        assert_eq!(tc.accuracy, CountAccuracy::Estimated);
        assert_eq!(tc.source, TokenCountSource::Heuristic);
    }

    #[test]
    fn token_count_exact_has_exact_accuracy() {
        let tc = TokenCount::exact(100, TokenCountSource::ExactTokenizer);
        assert_eq!(tc.tokens, 100);
        assert_eq!(tc.accuracy, CountAccuracy::Exact);
        assert_eq!(tc.source, TokenCountSource::ExactTokenizer);
    }

    #[test]
    fn token_count_zero_is_exact_identity() {
        let tc = TokenCount::zero();
        assert_eq!(tc.tokens, 0);
        assert_eq!(tc.accuracy, CountAccuracy::Exact);
    }

    // -- TokenCount arithmetic --

    #[test]
    fn add_sums_tokens() {
        let a = TokenCount::heuristic(10);
        let b = TokenCount::heuristic(20);
        assert_eq!((a + b).tokens, 30);
    }

    #[test]
    fn add_exact_plus_exact_stays_exact() {
        let a = TokenCount::exact(5, TokenCountSource::ExactTokenizer);
        let b = TokenCount::exact(3, TokenCountSource::ExactTokenizer);
        let sum = a + b;
        assert_eq!(sum.accuracy, CountAccuracy::Exact);
        assert_eq!(sum.source, TokenCountSource::ExactTokenizer);
    }

    #[test]
    fn add_exact_plus_estimated_degrades_to_estimated() {
        let a = TokenCount::exact(5, TokenCountSource::ExactTokenizer);
        let b = TokenCount::heuristic(3);
        assert_eq!((a + b).accuracy, CountAccuracy::Estimated);
    }

    #[test]
    fn add_mixed_sources_uses_least_reliable() {
        let a = TokenCount::exact(5, TokenCountSource::ProviderUsage);
        let b = TokenCount::exact(3, TokenCountSource::ExactTokenizer);
        // ExactTokenizer (rank 1) < ProviderUsage (rank 2)
        assert_eq!((a + b).source, TokenCountSource::ExactTokenizer);
    }

    #[test]
    fn add_zero_preserves_other_source() {
        let zero = TokenCount::zero();
        let real = TokenCount::exact(10, TokenCountSource::ProviderUsage);
        assert_eq!((zero + real).source, TokenCountSource::ProviderUsage);
        assert_eq!((real + zero).source, TokenCountSource::ProviderUsage);
    }

    #[test]
    fn add_assign_works() {
        let mut tc = TokenCount::heuristic(5);
        tc += TokenCount::heuristic(3);
        assert_eq!(tc.tokens, 8);
    }

    #[test]
    fn sum_over_iterator() {
        let counts = vec![
            TokenCount::heuristic(10),
            TokenCount::heuristic(20),
            TokenCount::heuristic(30),
        ];
        let total: TokenCount = counts.into_iter().sum();
        assert_eq!(total.tokens, 60);
        assert_eq!(total.accuracy, CountAccuracy::Estimated);
    }

    #[test]
    fn sum_empty_iterator_is_zero() {
        let total: TokenCount = std::iter::empty::<TokenCount>().sum();
        assert_eq!(total.tokens, 0);
        assert_eq!(total.accuracy, CountAccuracy::Exact);
    }

    // -- HeuristicTokenCounter --

    #[test]
    fn heuristic_count_text_empty_returns_zero() {
        let counter = HeuristicTokenCounter;
        let tc = counter.count_text("");
        assert_eq!(tc.tokens, 0);
        assert_eq!(tc.accuracy, CountAccuracy::Exact);
    }

    #[test]
    fn heuristic_count_text_short_string() {
        let counter = HeuristicTokenCounter;
        // 5 chars → ceil(5/4) = 2
        let tc = counter.count_text("hello");
        assert_eq!(tc.tokens, 2);
        assert_eq!(tc.accuracy, CountAccuracy::Estimated);
        assert_eq!(tc.source, TokenCountSource::Heuristic);
    }

    #[test]
    fn heuristic_count_message_matches_free_fn() {
        let msg = ModelMessage::user("hello world");
        let counter = HeuristicTokenCounter;
        assert_eq!(
            counter.count_message(&msg).tokens,
            estimate_message_tokens(&msg)
        );
    }

    #[test]
    fn heuristic_count_messages_sums_correctly() {
        let msgs = vec![
            ModelMessage::user("one"),
            ModelMessage::assistant("two"),
            ModelMessage::user("three"),
        ];
        let counter = HeuristicTokenCounter;
        let total = counter.count_messages(&msgs);
        let manual: usize = msgs.iter().map(|m| counter.count_message(m).tokens).sum();
        assert_eq!(total.tokens, manual);
    }

    // -- Backward-compatible free functions --

    #[test]
    fn estimate_text_tokens_empty_returns_zero() {
        assert_eq!(estimate_text_tokens(""), 0);
    }

    #[test]
    fn estimate_text_tokens_short_string() {
        // 5 chars → ceil(5/4) = 2
        assert_eq!(estimate_text_tokens("hello"), 2);
    }

    #[test]
    fn estimate_message_tokens_includes_overhead() {
        let msg = ModelMessage::user("hi");
        let tokens = estimate_message_tokens(&msg);
        // 3 framing + 1 role "user" + ceil(2/4) text = 3 + 1 + 1 = 5
        assert_eq!(tokens, 5);
    }

    #[test]
    fn estimate_context_usage_computes_remaining() {
        let messages = vec![ModelMessage::user("hello world")];
        let usage = estimate_context_usage(&messages, 1000);
        assert!(usage.used_tokens > 0);
        assert_eq!(usage.remaining_tokens, 1000 - usage.used_tokens);
        assert!(usage.usage_percent < 100);
    }

    #[test]
    fn estimate_context_usage_zero_window() {
        let messages = vec![ModelMessage::user("x")];
        let usage = estimate_context_usage(&messages, 0);
        assert_eq!(usage.usage_percent, 100);
        assert_eq!(usage.remaining_tokens, 0);
    }

    // -- Role contributes to count --

    #[test]
    fn different_roles_yield_different_counts() {
        let counter = HeuristicTokenCounter;
        let user_msg = ModelMessage::user("hello");
        let assistant_msg = ModelMessage::assistant("hello");
        let system_msg = ModelMessage::system("hello");

        let user_tokens = counter.count_message(&user_msg).tokens;
        let assistant_tokens = counter.count_message(&assistant_msg).tokens;
        let system_tokens = counter.count_message(&system_msg).tokens;

        // "user" (4 chars, 1 token) vs "assistant" (9 chars, 3 tokens)
        // vs "system" (6 chars, 2 tokens) — same content, different counts.
        assert!(
            assistant_tokens > user_tokens,
            "assistant ({assistant_tokens}) should cost more than user ({user_tokens})"
        );
        assert!(
            system_tokens > user_tokens,
            "system ({system_tokens}) should cost more than user ({user_tokens})"
        );
        assert_ne!(user_tokens, assistant_tokens);
    }

    // -- ContextUsageSnapshot: from_provider --

    #[test]
    fn snapshot_from_provider_is_high_confidence_exact() {
        let snap = ContextUsageSnapshot::from_provider(128_000, 50_000, 2_000);

        assert_eq!(snap.context_window, 128_000);
        assert_eq!(snap.provider_prompt_tokens, Some(50_000));
        assert_eq!(snap.provider_completion_tokens, Some(2_000));
        assert_eq!(snap.estimated_tail.tokens, 0);
        assert_eq!(snap.total_used.tokens, 52_000);
        assert_eq!(snap.total_used.accuracy, CountAccuracy::Exact);
        assert_eq!(snap.total_used.source, TokenCountSource::ProviderUsage);
        assert_eq!(snap.remaining_tokens, 128_000 - 52_000);
        assert_eq!(snap.confidence, SnapshotConfidence::High);
        assert_eq!(snap.source, SnapshotSource::ProviderReported);
        assert!(snap.has_provider_anchor());
        assert_eq!(snap.estimated_fraction(), 0.0);
    }

    #[test]
    fn snapshot_from_provider_usage_percent_correct() {
        let snap = ContextUsageSnapshot::from_provider(100_000, 75_000, 0);
        assert_eq!(snap.usage_percent, 75);
    }

    #[test]
    fn snapshot_from_provider_saturates_remaining() {
        // Provider reports usage exceeding window (possible with some APIs).
        let snap = ContextUsageSnapshot::from_provider(1000, 800, 500);
        assert_eq!(snap.remaining_tokens, 0);
        assert_eq!(snap.usage_percent, 100);
    }

    // -- ContextUsageSnapshot: from_provider_with_tail --

    #[test]
    fn snapshot_provider_with_tail_medium_confidence() {
        let tail = TokenCount::heuristic(500);
        let snap = ContextUsageSnapshot::from_provider_with_tail(128_000, 50_000, 2_000, tail);

        assert_eq!(snap.provider_prompt_tokens, Some(50_000));
        assert_eq!(snap.provider_completion_tokens, Some(2_000));
        assert_eq!(snap.estimated_tail.tokens, 500);
        assert_eq!(snap.total_used.tokens, 52_500);
        // Accuracy degrades to Estimated because of the heuristic tail.
        assert_eq!(snap.total_used.accuracy, CountAccuracy::Estimated);
        assert_eq!(snap.remaining_tokens, 128_000 - 52_500);
        assert_eq!(snap.confidence, SnapshotConfidence::Medium);
        assert_eq!(snap.source, SnapshotSource::ProviderReported);
        assert!(snap.has_provider_anchor());
    }

    #[test]
    fn snapshot_provider_with_zero_tail_is_high_confidence() {
        let tail = TokenCount::zero();
        let snap = ContextUsageSnapshot::from_provider_with_tail(128_000, 50_000, 2_000, tail);

        assert_eq!(snap.confidence, SnapshotConfidence::High);
        assert_eq!(snap.total_used.tokens, 52_000);
        assert_eq!(snap.estimated_tail.tokens, 0);
    }

    #[test]
    fn snapshot_estimated_fraction_with_tail() {
        let tail = TokenCount::heuristic(1_000);
        let snap = ContextUsageSnapshot::from_provider_with_tail(128_000, 9_000, 0, tail);

        // tail=1000, total=10000 → 0.1
        let frac = snap.estimated_fraction();
        assert!((frac - 0.1).abs() < f64::EPSILON);
    }

    // -- ContextUsageSnapshot: from_estimate --

    #[test]
    fn snapshot_from_estimate_low_confidence() {
        let estimated = TokenCount::heuristic(60_000);
        let snap = ContextUsageSnapshot::from_estimate(128_000, estimated);

        assert_eq!(snap.context_window, 128_000);
        assert!(snap.provider_prompt_tokens.is_none());
        assert!(snap.provider_completion_tokens.is_none());
        assert_eq!(snap.estimated_tail.tokens, 60_000);
        assert_eq!(snap.total_used.tokens, 60_000);
        assert_eq!(snap.total_used.accuracy, CountAccuracy::Estimated);
        assert_eq!(snap.remaining_tokens, 128_000 - 60_000);
        assert_eq!(snap.confidence, SnapshotConfidence::Low);
        assert_eq!(snap.source, SnapshotSource::FullEstimate);
        assert!(!snap.has_provider_anchor());
        assert_eq!(snap.estimated_fraction(), 1.0);
    }

    #[test]
    fn snapshot_from_estimate_zero_tokens_fraction_is_zero() {
        let snap = ContextUsageSnapshot::from_estimate(128_000, TokenCount::zero());
        assert_eq!(snap.estimated_fraction(), 0.0);
    }

    // -- ContextUsageSnapshot: to_context_usage --

    #[test]
    fn snapshot_to_context_usage_matches_legacy() {
        let snap = ContextUsageSnapshot::from_provider(128_000, 50_000, 2_000);
        let legacy = snap.to_context_usage();

        assert_eq!(legacy.used_tokens, 52_000);
        assert_eq!(legacy.context_window, 128_000);
        assert_eq!(legacy.remaining_tokens, 128_000 - 52_000);
        assert_eq!(legacy.usage_percent, snap.usage_percent);
    }

    #[test]
    fn snapshot_from_estimate_to_context_usage_round_trips() {
        let messages = vec![ModelMessage::user("hello world")];
        let legacy = estimate_context_usage(&messages, 1000);

        let estimated = TokenCount::heuristic(legacy.used_tokens);
        let snap = ContextUsageSnapshot::from_estimate(1000, estimated);
        let round_tripped = snap.to_context_usage();

        assert_eq!(round_tripped.used_tokens, legacy.used_tokens);
        assert_eq!(round_tripped.context_window, legacy.context_window);
        assert_eq!(round_tripped.remaining_tokens, legacy.remaining_tokens);
        assert_eq!(round_tripped.usage_percent, legacy.usage_percent);
    }

    // -- ContextUsageSnapshot: zero context window edge case --

    #[test]
    fn snapshot_zero_context_window_reports_100_percent() {
        let snap = ContextUsageSnapshot::from_provider(0, 100, 50);
        assert_eq!(snap.usage_percent, 100);
        assert_eq!(snap.remaining_tokens, 0);
    }

    // -- usage_pct helper --

    #[test]
    fn usage_pct_clamps_to_100() {
        assert_eq!(usage_pct(200, 100), 100);
    }

    #[test]
    fn usage_pct_zero_window_returns_100() {
        assert_eq!(usage_pct(0, 0), 100);
    }

    #[test]
    fn usage_pct_normal_case() {
        assert_eq!(usage_pct(50, 100), 50);
    }
}
