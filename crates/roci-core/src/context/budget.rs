//! Token-budget–aware message selection and budget foundation types.
//!
//! Utilities for choosing which messages fit within a given token budget,
//! biased toward keeping the most recent context.
//!
//! # Foundation types
//!
//! - [`ContextBudget`] — configuration for per-turn and per-session token limits
//! - [`BudgetSnapshot`] — preflight view of budget consumption for a pending request
//! - [`BudgetDecision`] — action to take when budget is evaluated

use crate::types::ModelMessage;

use super::tokens::estimate_message_tokens;

// ---------------------------------------------------------------------------
// Budget configuration
// ---------------------------------------------------------------------------

/// Configuration for context window token budgets.
///
/// Separates input and output budgets and supports per-turn and per-session
/// limits. The context window size can be overridden from the provider default.
///
/// Runtime enforcement is not yet wired — this type defines the configuration
/// surface that later compaction and overflow logic will consume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBudget {
    /// Override the provider-reported context window size.
    /// When `None`, the provider's reported context window is used.
    pub context_window_override: Option<usize>,
    /// Tokens reserved for model output in each turn.
    pub reserve_output_tokens: usize,
    /// Maximum input tokens per turn (`None` = no per-turn limit).
    pub max_turn_input_tokens: Option<usize>,
    /// Maximum cumulative input tokens across the session.
    pub max_session_input_tokens: Option<usize>,
    /// Maximum cumulative output tokens across the session.
    pub max_session_output_tokens: Option<usize>,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            context_window_override: None,
            reserve_output_tokens: 4096,
            max_turn_input_tokens: None,
            max_session_input_tokens: None,
            max_session_output_tokens: None,
        }
    }
}

impl ContextBudget {
    /// Resolve the effective context window size.
    ///
    /// Uses the override if set, otherwise the provider-reported window.
    pub fn effective_context_window(&self, provider_context_window: usize) -> usize {
        self.context_window_override
            .unwrap_or(provider_context_window)
    }

    /// Compute a preflight [`BudgetSnapshot`] before sending a request.
    ///
    /// # Parameters
    ///
    /// - `provider_context_window` — the model's reported context window size.
    /// - `turn_input_tokens` — input tokens for the request about to be sent.
    /// - `prior_session_input_tokens` — cumulative input tokens from all
    ///   *previous* turns (does **not** include the current turn).
    /// - `prior_session_output_tokens` — cumulative output tokens from all
    ///   *previous* turns.
    ///
    /// The snapshot projects session totals that would result from completing
    /// this turn: input projection adds `turn_input_tokens` to the prior
    /// total; output projection adds [`reserve_output_tokens`](Self::reserve_output_tokens)
    /// (the planned output budget) to the prior total.
    /// [`is_over_budget`](BudgetSnapshot::is_over_budget) evaluates against
    /// the projected totals so callers can reject or compact *before* the
    /// request is sent.
    pub fn snapshot(
        &self,
        provider_context_window: usize,
        turn_input_tokens: usize,
        prior_session_input_tokens: usize,
        prior_session_output_tokens: usize,
    ) -> BudgetSnapshot {
        let context_window = self.effective_context_window(provider_context_window);
        let reserved_output = self.reserve_output_tokens;

        // Turn input limit: min of window headroom and per-turn cap.
        let window_input_limit = context_window.saturating_sub(reserved_output);
        let turn_input_limit = match self.max_turn_input_tokens {
            Some(cap) => window_input_limit.min(cap),
            None => window_input_limit,
        };
        let turn_input_remaining = turn_input_limit.saturating_sub(turn_input_tokens);

        // Projected session totals (prior + this turn).
        let projected_session_input = prior_session_input_tokens + turn_input_tokens;
        let projected_session_output = prior_session_output_tokens + reserved_output;

        // Session limits (None = unconstrained).
        let session_input_limit = self.max_session_input_tokens;
        let session_input_remaining =
            session_input_limit.map(|lim| lim.saturating_sub(projected_session_input));

        let session_output_limit = self.max_session_output_tokens;
        let session_output_remaining =
            session_output_limit.map(|lim| lim.saturating_sub(projected_session_output));

        BudgetSnapshot {
            context_window,
            reserved_output,
            turn_input_limit,
            turn_input_used: turn_input_tokens,
            turn_input_remaining,
            session_input_limit,
            projected_session_input,
            session_input_remaining,
            session_output_limit,
            projected_session_output,
            session_output_remaining,
        }
    }
}

// ---------------------------------------------------------------------------
// Budget snapshot
// ---------------------------------------------------------------------------

/// Preflight view of budget consumption for a pending request.
///
/// Produced by [`ContextBudget::snapshot`]. Session-level fields carry
/// *projected* totals (prior turns + this turn's contribution) so that
/// [`is_over_budget`](Self::is_over_budget) can reject before the request
/// is sent. This is explicitly a pre-send check — not a post-turn record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetSnapshot {
    /// Total context window size in tokens.
    pub context_window: usize,
    /// Tokens reserved for model output.
    pub reserved_output: usize,

    // -- Turn-level input --
    /// Effective input cap for this turn: `min(context_window - reserved_output, max_turn_input_tokens)`.
    pub turn_input_limit: usize,
    /// Input tokens consumed by this turn's request.
    pub turn_input_used: usize,
    /// Input tokens still available for this turn.
    pub turn_input_remaining: usize,

    // -- Session-level input (projected) --
    /// Cumulative input token limit across the session (`None` = unconstrained).
    pub session_input_limit: Option<usize>,
    /// Projected cumulative input tokens after this turn completes
    /// (`prior_session_input + turn_input`).
    pub projected_session_input: usize,
    /// Session input tokens remaining after projection (`None` = unconstrained).
    pub session_input_remaining: Option<usize>,

    // -- Session-level output (projected) --
    /// Cumulative output token limit across the session (`None` = unconstrained).
    pub session_output_limit: Option<usize>,
    /// Projected cumulative output tokens after this turn completes
    /// (`prior_session_output + reserve_output_tokens`).
    pub projected_session_output: usize,
    /// Session output tokens remaining after projection (`None` = unconstrained).
    pub session_output_remaining: Option<usize>,
}

impl BudgetSnapshot {
    /// Percentage of the turn input limit currently consumed (0–100).
    pub fn usage_percent(&self) -> u8 {
        if self.turn_input_limit == 0 {
            return 100;
        }
        ((self.turn_input_used.saturating_mul(100)) / self.turn_input_limit).min(100) as u8
    }

    /// Whether any applicable limit (turn input, projected session input,
    /// projected session output) would be exceeded by this request.
    pub fn is_over_budget(&self) -> bool {
        if self.turn_input_used > self.turn_input_limit {
            return true;
        }
        if let Some(limit) = self.session_input_limit {
            if self.projected_session_input > limit {
                return true;
            }
        }
        if let Some(limit) = self.session_output_limit {
            if self.projected_session_output > limit {
                return true;
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Budget decision
// ---------------------------------------------------------------------------

/// Action to take after evaluating the current budget state.
///
/// Produced by budget-evaluation logic (not yet wired). Downstream consumers
/// (compaction, overflow, request pipeline) will pattern-match on this to
/// decide how to proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetDecision {
    /// Budget is within limits; proceed normally.
    Proceed,
    /// Context should be compacted to the target size.
    Compact {
        /// Target total input tokens after compaction.
        target_tokens: usize,
    },
    /// Reduce the max output tokens for this turn.
    ReduceMaxTokens {
        /// New max_tokens value to request from the provider.
        new_max_tokens: u32,
    },
    /// Reject the request entirely.
    Reject {
        /// Human-readable reason for rejection.
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// Existing message-selection utility
// ---------------------------------------------------------------------------

/// Select messages from the end of the slice that fit within `token_budget`.
///
/// Messages are considered newest-first; the returned `Vec` preserves
/// chronological order. Returns an empty `Vec` when the budget is zero or the
/// input is empty.
pub fn select_messages_with_token_budget_newest_first(
    messages: &[ModelMessage],
    token_budget: usize,
) -> Vec<ModelMessage> {
    if token_budget == 0 || messages.is_empty() {
        return Vec::new();
    }

    let mut selected = Vec::new();
    let mut used_tokens = 0usize;

    for message in messages.iter().rev() {
        let message_tokens = estimate_message_tokens(message);
        let next_tokens = used_tokens.saturating_add(message_tokens);
        if next_tokens > token_budget {
            break;
        }
        used_tokens = next_tokens;
        selected.push(message.clone());
    }

    selected.reverse();
    selected
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ModelMessage;

    // -- ContextBudget --

    #[test]
    fn default_budget_reserves_4096_output_tokens() {
        let budget = ContextBudget::default();
        assert_eq!(budget.reserve_output_tokens, 4096);
        assert!(budget.context_window_override.is_none());
        assert!(budget.max_turn_input_tokens.is_none());
        assert!(budget.max_session_input_tokens.is_none());
        assert!(budget.max_session_output_tokens.is_none());
    }

    #[test]
    fn effective_context_window_uses_provider_when_no_override() {
        let budget = ContextBudget::default();
        assert_eq!(budget.effective_context_window(128_000), 128_000);
    }

    #[test]
    fn effective_context_window_uses_override_when_set() {
        let budget = ContextBudget {
            context_window_override: Some(64_000),
            ..Default::default()
        };
        assert_eq!(budget.effective_context_window(128_000), 64_000);
    }

    // -- Preflight snapshot: turn-level --

    #[test]
    fn snapshot_computes_turn_input_limit() {
        let budget = ContextBudget {
            reserve_output_tokens: 4096,
            ..Default::default()
        };
        let snap = budget.snapshot(128_000, 50_000, 0, 0);
        assert_eq!(snap.context_window, 128_000);
        assert_eq!(snap.reserved_output, 4096);
        assert_eq!(snap.turn_input_limit, 128_000 - 4096);
        assert_eq!(snap.turn_input_used, 50_000);
        assert_eq!(snap.turn_input_remaining, 128_000 - 4096 - 50_000);
    }

    #[test]
    fn snapshot_with_context_window_override() {
        let budget = ContextBudget {
            context_window_override: Some(32_000),
            reserve_output_tokens: 2048,
            ..Default::default()
        };
        let snap = budget.snapshot(128_000, 10_000, 0, 0);
        assert_eq!(snap.context_window, 32_000);
        assert_eq!(snap.turn_input_limit, 32_000 - 2048);
        assert_eq!(snap.turn_input_remaining, 32_000 - 2048 - 10_000);
    }

    #[test]
    fn snapshot_saturates_when_turn_usage_exceeds_limit() {
        let budget = ContextBudget {
            context_window_override: Some(1000),
            reserve_output_tokens: 500,
            ..Default::default()
        };
        let snap = budget.snapshot(1000, 600, 0, 0);
        assert_eq!(snap.turn_input_remaining, 0);
    }

    // -- Preflight snapshot: session projection --

    #[test]
    fn snapshot_projects_session_input_as_prior_plus_turn() {
        let budget = ContextBudget {
            max_session_input_tokens: Some(500_000),
            ..Default::default()
        };
        // prior=200_000, turn=10_000 → projected=210_000
        let snap = budget.snapshot(128_000, 10_000, 200_000, 0);
        assert_eq!(snap.projected_session_input, 210_000);
        assert_eq!(snap.session_input_remaining, Some(500_000 - 210_000));
    }

    #[test]
    fn snapshot_projects_session_output_as_prior_plus_reserve() {
        let budget = ContextBudget {
            reserve_output_tokens: 4096,
            max_session_output_tokens: Some(100_000),
            ..Default::default()
        };
        // prior=40_000 → projected=40_000+4096=44_096
        let snap = budget.snapshot(128_000, 10_000, 0, 40_000);
        assert_eq!(snap.projected_session_output, 44_096);
        assert_eq!(snap.session_output_remaining, Some(100_000 - 44_096));
    }

    #[test]
    fn snapshot_session_fields_none_when_unconstrained() {
        let budget = ContextBudget::default();
        let snap = budget.snapshot(128_000, 10_000, 999_999, 888_888);
        assert_eq!(snap.session_input_limit, None);
        assert_eq!(snap.session_input_remaining, None);
        assert_eq!(snap.session_output_limit, None);
        assert_eq!(snap.session_output_remaining, None);
        // Projected totals are still computed even without limits.
        assert_eq!(snap.projected_session_input, 999_999 + 10_000);
        assert_eq!(snap.projected_session_output, 888_888 + 4096);
    }

    // -- Turn-input cap --

    #[test]
    fn max_turn_input_tokens_caps_large_model_window() {
        let budget = ContextBudget {
            reserve_output_tokens: 4096,
            max_turn_input_tokens: Some(8000),
            ..Default::default()
        };
        // window headroom = 123_904, but turn cap is 8000
        let snap = budget.snapshot(128_000, 5000, 0, 0);
        assert_eq!(snap.turn_input_limit, 8000);
        assert_eq!(snap.turn_input_remaining, 3000);
        assert!(!snap.is_over_budget());

        // Over the turn cap
        let snap_over = budget.snapshot(128_000, 9000, 0, 0);
        assert!(snap_over.is_over_budget());
    }

    #[test]
    fn max_turn_input_tokens_no_effect_when_window_is_smaller() {
        let budget = ContextBudget {
            context_window_override: Some(10_000),
            reserve_output_tokens: 4000,
            max_turn_input_tokens: Some(50_000),
            ..Default::default()
        };
        let snap = budget.snapshot(128_000, 3000, 0, 0);
        assert_eq!(snap.turn_input_limit, 6000);
    }

    // -- Session-input enforcement (preflight) --

    #[test]
    fn max_session_input_tokens_enforced_via_projection() {
        let budget = ContextBudget {
            max_session_input_tokens: Some(100_000),
            ..Default::default()
        };
        // prior=89_999 + turn=10_000 = 99_999 ≤ 100_000 → OK
        let snap_ok = budget.snapshot(128_000, 10_000, 89_999, 0);
        assert!(!snap_ok.is_over_budget());
        assert_eq!(snap_ok.projected_session_input, 99_999);

        // prior=90_001 + turn=10_000 = 100_001 > 100_000 → over
        let snap_over = budget.snapshot(128_000, 10_000, 90_001, 0);
        assert!(snap_over.is_over_budget());
        assert_eq!(snap_over.projected_session_input, 100_001);
    }

    // -- Session-output enforcement (preflight) --

    #[test]
    fn max_session_output_tokens_enforced_via_projection() {
        let budget = ContextBudget {
            reserve_output_tokens: 4096,
            max_session_output_tokens: Some(50_000),
            ..Default::default()
        };
        // prior=45_903 + reserve=4096 = 49_999 ≤ 50_000 → OK
        let snap_ok = budget.snapshot(128_000, 10_000, 0, 45_903);
        assert!(!snap_ok.is_over_budget());
        assert_eq!(snap_ok.projected_session_output, 49_999);

        // prior=45_905 + reserve=4096 = 50_001 > 50_000 → over
        let snap_over = budget.snapshot(128_000, 10_000, 0, 45_905);
        assert!(snap_over.is_over_budget());
        assert_eq!(snap_over.projected_session_output, 50_001);
    }

    // -- usage_percent --

    #[test]
    fn usage_percent_correct() {
        let budget = ContextBudget {
            context_window_override: Some(10_000),
            reserve_output_tokens: 2000,
            ..Default::default()
        };
        let snap = budget.snapshot(10_000, 4000, 0, 0);
        assert_eq!(snap.usage_percent(), 50);
    }

    #[test]
    fn usage_percent_zero_limit_returns_100() {
        let budget = ContextBudget {
            context_window_override: Some(0),
            reserve_output_tokens: 0,
            ..Default::default()
        };
        let snap = budget.snapshot(0, 0, 0, 0);
        assert_eq!(snap.usage_percent(), 100);
    }

    // -- is_over_budget edge cases --

    #[test]
    fn is_not_over_budget_at_exact_limits() {
        let budget = ContextBudget {
            context_window_override: Some(1000),
            reserve_output_tokens: 500,
            max_session_input_tokens: Some(10_000),
            max_session_output_tokens: Some(5_000),
            ..Default::default()
        };
        // turn=500 (= limit), projected_input=9_500+500=10_000, projected_output=4_500+500=5_000
        let snap = budget.snapshot(1000, 500, 9_500, 4_500);
        assert!(!snap.is_over_budget());
    }

    #[test]
    fn is_over_budget_from_turn_input_only() {
        let budget = ContextBudget {
            context_window_override: Some(1000),
            reserve_output_tokens: 500,
            ..Default::default()
        };
        let snap = budget.snapshot(1000, 501, 0, 0);
        assert!(snap.is_over_budget());
    }

    // -- BudgetDecision --

    #[test]
    fn budget_decision_variants_construct() {
        let _ = BudgetDecision::Proceed;
        let _ = BudgetDecision::Compact {
            target_tokens: 50_000,
        };
        let _ = BudgetDecision::ReduceMaxTokens {
            new_max_tokens: 2048,
        };
        let _ = BudgetDecision::Reject {
            reason: "session limit exceeded".to_string(),
        };
    }

    #[test]
    fn budget_decision_eq() {
        assert_eq!(BudgetDecision::Proceed, BudgetDecision::Proceed);
        assert_eq!(
            BudgetDecision::Compact {
                target_tokens: 1000
            },
            BudgetDecision::Compact {
                target_tokens: 1000
            },
        );
        assert_ne!(
            BudgetDecision::Compact {
                target_tokens: 1000
            },
            BudgetDecision::Compact {
                target_tokens: 2000
            },
        );
    }

    // -- Existing message-selection tests --

    #[test]
    fn selects_newest_messages_within_budget() {
        let oldest = ModelMessage::user("oldest");
        let middle = ModelMessage::assistant("middle");
        let newest = ModelMessage::user("newest message");
        let messages = vec![oldest, middle, newest.clone()];
        let budget = estimate_message_tokens(&newest) + estimate_message_tokens(&messages[1]);

        let selected = select_messages_with_token_budget_newest_first(&messages, budget);

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].text(), "middle");
        assert_eq!(selected[1].text(), "newest message");
    }

    #[test]
    fn zero_budget_returns_empty() {
        let messages = vec![ModelMessage::user("hello")];
        assert!(select_messages_with_token_budget_newest_first(&messages, 0).is_empty());
    }

    #[test]
    fn empty_messages_returns_empty() {
        assert!(select_messages_with_token_budget_newest_first(&[], 1000).is_empty());
    }
}
