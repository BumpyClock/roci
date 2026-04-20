//! Overflow recovery policy and event contracts.
//!
//! Defines the deterministic recovery ladder for context-window overflows:
//! one optional output-budget reduction, then up to two compaction attempts
//! (second gated on meaningful progress from the first). This module owns
//! only provider-agnostic types and policy math — actual execution wiring
//! is deferred to the runner.
//!
//! # Recovery ladder (fixed)
//!
//! 1. **Output budget reduction** — attempted once when the signal's
//!    [`OverflowRetryHint`] recommends it and it hasn't been tried yet.
//! 2. **First compaction** — either as the direct first step or after an
//!    output reduction proves insufficient.
//! 3. **Second compaction** — only if the first freed at least
//!    [`MIN_PROGRESS_TOKENS`] tokens.
//! 4. **Abort** — all recovery options exhausted.
//!
//! The ladder constants are locked in this module. There is no public
//! configuration surface — the policy encodes exactly the behavior above.
//!
//! # Separation from generic retry
//!
//! Overflow recovery is a distinct concern from transient-error retry and
//! rate-limit backoff. The runner must not conflate overflow recovery
//! attempts with generic retry budgets.

use super::overflow::{OverflowKind, OverflowRetryHint, OverflowSignal};

// ---------------------------------------------------------------------------
// Constants (locked ladder)
// ---------------------------------------------------------------------------

/// Maximum compaction attempts per overflow recovery episode.
const MAX_COMPACTION_ATTEMPTS: u8 = 2;

/// Maximum total recovery attempts per episode:
/// one output-budget reduction plus two compactions.
const MAX_TOTAL_ATTEMPTS: u8 = 1 + MAX_COMPACTION_ATTEMPTS;

/// Minimum tokens freed to justify a subsequent compaction attempt.
///
/// If a compaction frees fewer tokens than this threshold, the policy
/// aborts rather than attempting another round of compaction.
const MIN_PROGRESS_TOKENS: usize = 500;

// ---------------------------------------------------------------------------
// Recovery actions
// ---------------------------------------------------------------------------

/// Action the recovery policy recommends as the next step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RecoveryAction {
    /// Reduce the output-token budget and retry the request.
    ReduceOutputBudget,
    /// Compact the conversation context and retry the request.
    CompactContext,
    /// No further automatic recovery is possible; surface the error.
    Abort,
}

impl RecoveryAction {
    /// Whether this action represents a recovery attempt (not an abort).
    pub fn is_recovery(self) -> bool {
        !matches!(self, Self::Abort)
    }
}

// ---------------------------------------------------------------------------
// Recovery reasons
// ---------------------------------------------------------------------------

/// Why the policy chose a particular [`RecoveryAction`].
///
/// Each variant maps to exactly one action kind. The action is derived
/// from the reason — callers cannot construct mismatched pairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RecoveryReason {
    // -- ReduceOutputBudget reasons ------------------------------------------
    /// Output overflow detected and budget reduction has not been attempted.
    OutputBudgetReductionAvailable,

    // -- CompactContext reasons ----------------------------------------------
    /// The overflow signal recommends compaction as the first recovery step.
    CompactionRequired,
    /// A prior output-budget reduction did not resolve the overflow.
    OutputReductionInsufficient,
    /// The previous compaction freed sufficient tokens to justify another.
    CompactionProgressSufficient,

    // -- Abort reasons -------------------------------------------------------
    /// The overflow signal does not support automatic recovery.
    NotRecoverable,
    /// All allowed compaction attempts have been used.
    CompactionAttemptsExhausted,
    /// The last compaction did not free enough tokens for another attempt.
    CompactionProgressInsufficient,
}

impl RecoveryReason {
    /// The action implied by this reason.
    ///
    /// The mapping is fixed: each reason variant always produces the same
    /// action, ensuring the reason/action invariant holds by construction.
    pub fn action(self) -> RecoveryAction {
        match self {
            Self::OutputBudgetReductionAvailable => RecoveryAction::ReduceOutputBudget,
            Self::CompactionRequired
            | Self::OutputReductionInsufficient
            | Self::CompactionProgressSufficient => RecoveryAction::CompactContext,
            Self::NotRecoverable
            | Self::CompactionAttemptsExhausted
            | Self::CompactionProgressInsufficient => RecoveryAction::Abort,
        }
    }

    /// Whether this reason corresponds to an abort decision.
    pub fn is_abort(self) -> bool {
        self.action() == RecoveryAction::Abort
    }

    /// Convert to an [`AbortReason`] if this is a terminal reason.
    ///
    /// Returns `None` for non-terminal (recovery) reasons.
    pub fn as_abort_reason(self) -> Option<AbortReason> {
        match self {
            Self::NotRecoverable => Some(AbortReason::NotRecoverable),
            Self::CompactionAttemptsExhausted => Some(AbortReason::CompactionAttemptsExhausted),
            Self::CompactionProgressInsufficient => {
                Some(AbortReason::CompactionProgressInsufficient)
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Abort reasons (terminal-only subset)
// ---------------------------------------------------------------------------

/// Terminal reason for aborting an overflow recovery episode.
///
/// A strict subset of [`RecoveryReason`] containing only abort-worthy
/// variants. Used by [`RecoveryEvent::EpisodeExhausted`] to make invalid
/// exhaustion events unrepresentable at the type level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AbortReason {
    /// The overflow signal does not support automatic recovery.
    NotRecoverable,
    /// All allowed compaction attempts have been used.
    CompactionAttemptsExhausted,
    /// The last compaction did not free enough tokens for another attempt.
    CompactionProgressInsufficient,
}

impl AbortReason {
    /// Convert to the corresponding [`RecoveryReason`].
    pub fn as_recovery_reason(self) -> RecoveryReason {
        match self {
            Self::NotRecoverable => RecoveryReason::NotRecoverable,
            Self::CompactionAttemptsExhausted => RecoveryReason::CompactionAttemptsExhausted,
            Self::CompactionProgressInsufficient => RecoveryReason::CompactionProgressInsufficient,
        }
    }
}

impl From<AbortReason> for RecoveryReason {
    fn from(abort: AbortReason) -> Self {
        abort.as_recovery_reason()
    }
}

// ---------------------------------------------------------------------------
// Recovery decision
// ---------------------------------------------------------------------------

/// Combined action and reason produced by [`OverflowRecoveryPolicy::next_action`].
///
/// The action is derived from the reason — impossible pairs (e.g.
/// `Abort` + `CompactionRequired`) cannot be constructed. Use
/// [`action()`](Self::action) and [`reason()`](Self::reason) accessors
/// to inspect the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryDecision {
    reason: RecoveryReason,
}

impl RecoveryDecision {
    /// Create a decision from a reason. The action is derived automatically.
    fn from_reason(reason: RecoveryReason) -> Self {
        Self { reason }
    }

    /// The recommended action.
    pub fn action(&self) -> RecoveryAction {
        self.reason.action()
    }

    /// Why this action was chosen.
    pub fn reason(&self) -> RecoveryReason {
        self.reason
    }

    /// Whether the decision is to continue recovery (not abort).
    pub fn is_recovery(&self) -> bool {
        self.action().is_recovery()
    }

    /// Convert this decision's reason to an [`AbortReason`], if terminal.
    pub fn as_abort_reason(&self) -> Option<AbortReason> {
        self.reason.as_abort_reason()
    }
}

// ---------------------------------------------------------------------------
// Compaction progress
// ---------------------------------------------------------------------------

/// Token-count delta from a single compaction attempt.
///
/// Used by the policy to gate subsequent compaction retries on meaningful
/// progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionProgress {
    /// Token count before the compaction was applied.
    pub tokens_before: usize,
    /// Token count after the compaction was applied.
    pub tokens_after: usize,
}

impl CompactionProgress {
    /// Create a new progress record.
    pub fn new(tokens_before: usize, tokens_after: usize) -> Self {
        Self {
            tokens_before,
            tokens_after,
        }
    }

    /// Tokens freed by this compaction (saturating subtraction).
    pub fn tokens_freed(&self) -> usize {
        self.tokens_before.saturating_sub(self.tokens_after)
    }

    /// Whether the compaction met a minimum token-reduction threshold.
    pub fn meets_threshold(&self, min_tokens: usize) -> bool {
        self.tokens_freed() >= min_tokens
    }
}

// ---------------------------------------------------------------------------
// Recovery state
// ---------------------------------------------------------------------------

/// Mutable state tracking for a single overflow recovery episode.
///
/// Created when an overflow is first detected and updated as recovery
/// actions are applied. The policy reads this state to decide the next
/// action; the runtime mutates it between decisions.
///
/// All internal counters use saturating arithmetic capped at the fixed
/// ladder limits. Under normal operation the caps are never hit; they
/// exist as a safety net against misuse.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryState {
    /// Whether an output-budget reduction has been attempted.
    output_reduction_attempted: bool,
    /// Number of compaction attempts completed so far.
    ///
    /// Capped at [`MAX_COMPACTION_ATTEMPTS`] via saturating increment.
    compaction_attempts: u8,
    /// Progress from the most recent compaction attempt.
    last_compaction_progress: Option<CompactionProgress>,
}

impl RecoveryState {
    /// Create a fresh state for a new recovery episode.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that an output-budget reduction was attempted.
    pub fn record_output_reduction(&mut self) {
        self.output_reduction_attempted = true;
    }

    /// Record a completed compaction attempt with its progress.
    ///
    /// The compaction counter saturates at [`MAX_COMPACTION_ATTEMPTS`]
    /// to prevent overflow under any call pattern.
    pub fn record_compaction(&mut self, progress: CompactionProgress) {
        self.compaction_attempts = self
            .compaction_attempts
            .saturating_add(1)
            .min(MAX_COMPACTION_ATTEMPTS);
        self.last_compaction_progress = Some(progress);
    }

    /// Whether an output-budget reduction has been attempted.
    pub fn output_reduction_attempted(&self) -> bool {
        self.output_reduction_attempted
    }

    /// Number of compaction attempts completed.
    pub fn compaction_attempts(&self) -> u8 {
        self.compaction_attempts
    }

    /// Progress from the last compaction, if any.
    pub fn last_compaction_progress(&self) -> Option<&CompactionProgress> {
        self.last_compaction_progress.as_ref()
    }

    /// Total recovery attempts (output reductions + compactions).
    ///
    /// Saturates at the fixed ladder maximum rather than wrapping.
    pub fn total_attempts(&self) -> u8 {
        let output = u8::from(self.output_reduction_attempted);
        output
            .saturating_add(self.compaction_attempts)
            .min(MAX_TOTAL_ATTEMPTS)
    }
}

// ---------------------------------------------------------------------------
// Recovery events (contract for later runtime wiring)
// ---------------------------------------------------------------------------

/// Lifecycle events for overflow recovery episodes.
///
/// Emitted by the runtime at each stage of a recovery attempt. The types
/// are defined here to establish the contract before runtime wiring.
///
/// `EpisodeExhausted` accepts only [`AbortReason`] — non-terminal reasons
/// are unrepresentable at the type level.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecoveryEvent {
    /// An overflow was detected and a recovery episode is starting.
    EpisodeStarted {
        /// The classified overflow category.
        overflow_kind: OverflowKind,
    },

    /// The policy produced a recovery decision.
    ActionDecided {
        /// The recommended action and reason.
        decision: RecoveryDecision,
        /// Zero-based index of this attempt within the episode.
        attempt_index: u8,
    },

    /// The overflow was resolved after recovery actions.
    EpisodeResolved {
        /// Total recovery attempts made before resolution.
        total_attempts: u8,
    },

    /// Recovery was exhausted without resolving the overflow.
    EpisodeExhausted {
        /// Why recovery could not continue (terminal reasons only).
        reason: AbortReason,
        /// Total recovery attempts made before exhaustion.
        total_attempts: u8,
    },
}

// ---------------------------------------------------------------------------
// The policy
// ---------------------------------------------------------------------------

/// Deterministic overflow recovery policy with a fixed ladder.
///
/// Given an [`OverflowSignal`] and the current [`RecoveryState`], produces
/// the next [`RecoveryDecision`]. The policy is pure — it performs no I/O
/// or mutation. The runtime is responsible for applying the decision and
/// updating the state between calls.
///
/// # Fixed recovery ladder
///
/// 1. **Output budget reduction** — attempted once when the signal's
///    [`OverflowRetryHint`] recommends it and it hasn't been tried.
/// 2. **First compaction** — either as the direct first step or after
///    output reduction proves insufficient.
/// 3. **Second compaction** — only if the first compaction freed at least
///    [`MIN_PROGRESS_TOKENS`] tokens.
/// 4. **Abort** — when all options are exhausted.
///
/// There is no public configuration surface. The ladder is locked to the
/// constants defined in this module.
#[derive(Debug, Clone)]
pub struct OverflowRecoveryPolicy {
    max_compaction_attempts: u8,
    min_progress_tokens: usize,
}

impl Default for OverflowRecoveryPolicy {
    fn default() -> Self {
        Self {
            max_compaction_attempts: MAX_COMPACTION_ATTEMPTS,
            min_progress_tokens: MIN_PROGRESS_TOKENS,
        }
    }
}

impl OverflowRecoveryPolicy {
    /// Create a policy with the fixed recovery ladder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Maximum compaction attempts per episode (fixed at 2).
    pub fn max_compaction_attempts(&self) -> u8 {
        self.max_compaction_attempts
    }

    /// Minimum tokens a compaction must free to justify another attempt.
    pub fn min_progress_tokens(&self) -> usize {
        self.min_progress_tokens
    }

    /// Test-only constructor for verifying edge cases with non-default
    /// ladder parameters.
    #[cfg(test)]
    fn with_test_config(max_compaction_attempts: u8, min_progress_tokens: usize) -> Self {
        Self {
            max_compaction_attempts,
            min_progress_tokens,
        }
    }

    /// Determine the next recovery action given the overflow signal and
    /// current recovery state.
    ///
    /// This is a pure function: it reads the signal and state but performs
    /// no side effects. The caller must apply the decision and update the
    /// state via [`RecoveryState::record_output_reduction`] or
    /// [`RecoveryState::record_compaction`] before calling again.
    pub fn next_action(&self, signal: &OverflowSignal, state: &RecoveryState) -> RecoveryDecision {
        // 1. Non-recoverable signals abort immediately.
        if !signal.is_recoverable() {
            return RecoveryDecision::from_reason(RecoveryReason::NotRecoverable);
        }

        // 2. Attempt output-budget reduction first when recommended and untried.
        if signal.retry_hint == OverflowRetryHint::ReduceOutputTokensFirst
            && !state.output_reduction_attempted
        {
            return RecoveryDecision::from_reason(RecoveryReason::OutputBudgetReductionAvailable);
        }

        // 3. Compaction ladder — check limit before allowing any attempt.
        if state.compaction_attempts >= self.max_compaction_attempts {
            return RecoveryDecision::from_reason(RecoveryReason::CompactionAttemptsExhausted);
        }

        if state.compaction_attempts == 0 {
            // First compaction: pick the reason based on prior actions.
            let reason = if state.output_reduction_attempted {
                RecoveryReason::OutputReductionInsufficient
            } else {
                RecoveryReason::CompactionRequired
            };
            RecoveryDecision::from_reason(reason)
        } else {
            // Subsequent compaction: gate on meaningful progress.
            let has_progress = state
                .last_compaction_progress
                .as_ref()
                .is_some_and(|p| p.meets_threshold(self.min_progress_tokens));

            if has_progress {
                RecoveryDecision::from_reason(RecoveryReason::CompactionProgressSufficient)
            } else {
                RecoveryDecision::from_reason(RecoveryReason::CompactionProgressInsufficient)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Helpers ------------------------------------------------------------

    fn output_overflow_signal() -> OverflowSignal {
        OverflowSignal::new(
            OverflowKind::OutputOverflow,
            OverflowRetryHint::ReduceOutputTokensFirst,
        )
    }

    fn input_overflow_signal() -> OverflowSignal {
        OverflowSignal::new(
            OverflowKind::InputOverflow,
            OverflowRetryHint::CompactContextFirst,
        )
    }

    fn non_recoverable_signal() -> OverflowSignal {
        OverflowSignal::new(
            OverflowKind::UnknownOverflow,
            OverflowRetryHint::NoAutomaticRecovery,
        )
    }

    fn policy() -> OverflowRecoveryPolicy {
        OverflowRecoveryPolicy::new()
    }

    fn progress_with_freed(freed: usize) -> CompactionProgress {
        CompactionProgress::new(10_000, 10_000 - freed)
    }

    // -- RecoveryAction -----------------------------------------------------

    #[test]
    fn recovery_actions_report_is_recovery_correctly() {
        assert!(RecoveryAction::ReduceOutputBudget.is_recovery());
        assert!(RecoveryAction::CompactContext.is_recovery());
        assert!(!RecoveryAction::Abort.is_recovery());
    }

    // -- RecoveryReason: action derivation ----------------------------------

    #[test]
    fn reason_derives_reduce_output_budget_action() {
        assert_eq!(
            RecoveryReason::OutputBudgetReductionAvailable.action(),
            RecoveryAction::ReduceOutputBudget,
        );
    }

    #[test]
    fn reason_derives_compact_context_action() {
        assert_eq!(
            RecoveryReason::CompactionRequired.action(),
            RecoveryAction::CompactContext,
        );
        assert_eq!(
            RecoveryReason::OutputReductionInsufficient.action(),
            RecoveryAction::CompactContext,
        );
        assert_eq!(
            RecoveryReason::CompactionProgressSufficient.action(),
            RecoveryAction::CompactContext,
        );
    }

    #[test]
    fn reason_derives_abort_action() {
        assert_eq!(
            RecoveryReason::NotRecoverable.action(),
            RecoveryAction::Abort,
        );
        assert_eq!(
            RecoveryReason::CompactionAttemptsExhausted.action(),
            RecoveryAction::Abort,
        );
        assert_eq!(
            RecoveryReason::CompactionProgressInsufficient.action(),
            RecoveryAction::Abort,
        );
    }

    // -- RecoveryReason: is_abort -------------------------------------------

    #[test]
    fn abort_reasons_report_is_abort() {
        assert!(RecoveryReason::NotRecoverable.is_abort());
        assert!(RecoveryReason::CompactionAttemptsExhausted.is_abort());
        assert!(RecoveryReason::CompactionProgressInsufficient.is_abort());
    }

    #[test]
    fn non_abort_reasons_are_not_abort() {
        assert!(!RecoveryReason::OutputBudgetReductionAvailable.is_abort());
        assert!(!RecoveryReason::CompactionRequired.is_abort());
        assert!(!RecoveryReason::OutputReductionInsufficient.is_abort());
        assert!(!RecoveryReason::CompactionProgressSufficient.is_abort());
    }

    // -- RecoveryReason: as_abort_reason ------------------------------------

    #[test]
    fn abort_reasons_convert_to_abort_reason() {
        assert_eq!(
            RecoveryReason::NotRecoverable.as_abort_reason(),
            Some(AbortReason::NotRecoverable),
        );
        assert_eq!(
            RecoveryReason::CompactionAttemptsExhausted.as_abort_reason(),
            Some(AbortReason::CompactionAttemptsExhausted),
        );
        assert_eq!(
            RecoveryReason::CompactionProgressInsufficient.as_abort_reason(),
            Some(AbortReason::CompactionProgressInsufficient),
        );
    }

    #[test]
    fn non_abort_reasons_do_not_convert() {
        assert!(RecoveryReason::OutputBudgetReductionAvailable
            .as_abort_reason()
            .is_none());
        assert!(RecoveryReason::CompactionRequired
            .as_abort_reason()
            .is_none());
        assert!(RecoveryReason::OutputReductionInsufficient
            .as_abort_reason()
            .is_none());
        assert!(RecoveryReason::CompactionProgressSufficient
            .as_abort_reason()
            .is_none());
    }

    // -- AbortReason --------------------------------------------------------

    #[test]
    fn abort_reason_roundtrips_through_recovery_reason() {
        let cases = [
            AbortReason::NotRecoverable,
            AbortReason::CompactionAttemptsExhausted,
            AbortReason::CompactionProgressInsufficient,
        ];
        for abort in cases {
            let reason: RecoveryReason = abort.into();
            assert_eq!(reason.as_abort_reason(), Some(abort));
        }
    }

    // -- RecoveryDecision ---------------------------------------------------

    #[test]
    fn decision_action_is_derived_from_reason() {
        let d = RecoveryDecision::from_reason(RecoveryReason::CompactionRequired);
        assert_eq!(d.action(), RecoveryAction::CompactContext);
        assert_eq!(d.reason(), RecoveryReason::CompactionRequired);
        assert!(d.is_recovery());
    }

    #[test]
    fn decision_abort_exposes_abort_reason() {
        let d = RecoveryDecision::from_reason(RecoveryReason::NotRecoverable);
        assert_eq!(d.action(), RecoveryAction::Abort);
        assert!(!d.is_recovery());
        assert_eq!(d.as_abort_reason(), Some(AbortReason::NotRecoverable));
    }

    #[test]
    fn decision_recovery_has_no_abort_reason() {
        let d = RecoveryDecision::from_reason(RecoveryReason::OutputBudgetReductionAvailable);
        assert!(d.as_abort_reason().is_none());
    }

    // -- CompactionProgress -------------------------------------------------

    #[test]
    fn compaction_progress_reports_tokens_freed() {
        let p = CompactionProgress::new(10_000, 7_000);
        assert_eq!(p.tokens_freed(), 3_000);
    }

    #[test]
    fn compaction_progress_saturates_when_after_exceeds_before() {
        let p = CompactionProgress::new(5_000, 8_000);
        assert_eq!(p.tokens_freed(), 0);
    }

    #[test]
    fn compaction_progress_meets_threshold() {
        let p = CompactionProgress::new(10_000, 9_000);
        assert!(p.meets_threshold(1_000));
        assert!(p.meets_threshold(500));
        assert!(!p.meets_threshold(1_001));
    }

    #[test]
    fn compaction_progress_zero_freed_fails_any_nonzero_threshold() {
        let p = CompactionProgress::new(5_000, 5_000);
        assert!(!p.meets_threshold(1));
        assert!(p.meets_threshold(0));
    }

    // -- RecoveryState ------------------------------------------------------

    #[test]
    fn fresh_state_has_zero_attempts() {
        let state = RecoveryState::new();
        assert!(!state.output_reduction_attempted());
        assert_eq!(state.compaction_attempts(), 0);
        assert!(state.last_compaction_progress().is_none());
        assert_eq!(state.total_attempts(), 0);
    }

    #[test]
    fn state_tracks_output_reduction() {
        let mut state = RecoveryState::new();
        state.record_output_reduction();
        assert!(state.output_reduction_attempted());
        assert_eq!(state.total_attempts(), 1);
    }

    #[test]
    fn state_tracks_compaction_attempts() {
        let mut state = RecoveryState::new();
        let progress = CompactionProgress::new(10_000, 8_000);
        state.record_compaction(progress);
        assert_eq!(state.compaction_attempts(), 1);
        assert_eq!(state.last_compaction_progress(), Some(&progress));
        assert_eq!(state.total_attempts(), 1);
    }

    #[test]
    fn state_total_attempts_sums_all_actions() {
        let mut state = RecoveryState::new();
        state.record_output_reduction();
        state.record_compaction(CompactionProgress::new(10_000, 7_000));
        state.record_compaction(CompactionProgress::new(7_000, 5_000));
        assert_eq!(state.total_attempts(), 3);
    }

    #[test]
    fn state_compaction_counter_saturates() {
        let mut state = RecoveryState::new();
        for _ in 0..u8::MAX as u16 + 10 {
            state.record_compaction(CompactionProgress::new(1000, 500));
        }
        // Must not have wrapped or exceeded the fixed ladder.
        assert_eq!(state.compaction_attempts(), MAX_COMPACTION_ATTEMPTS);
        // total_attempts also saturates at the ladder maximum.
        state.record_output_reduction();
        assert_eq!(state.total_attempts(), MAX_TOTAL_ATTEMPTS);
    }

    // -- Policy: fixed ladder constants -------------------------------------

    #[test]
    fn policy_reports_fixed_ladder_constants() {
        let p = policy();
        assert_eq!(p.max_compaction_attempts(), 2);
        assert_eq!(p.min_progress_tokens(), 500);
    }

    // -- Policy: non-recoverable signal -------------------------------------

    #[test]
    fn non_recoverable_signal_aborts_immediately() {
        let d = policy().next_action(&non_recoverable_signal(), &RecoveryState::new());
        assert_eq!(d.action(), RecoveryAction::Abort);
        assert_eq!(d.reason(), RecoveryReason::NotRecoverable);
    }

    #[test]
    fn non_recoverable_aborts_even_with_prior_work() {
        let mut state = RecoveryState::new();
        state.record_output_reduction();
        let d = policy().next_action(&non_recoverable_signal(), &state);
        assert_eq!(d.action(), RecoveryAction::Abort);
        assert_eq!(d.reason(), RecoveryReason::NotRecoverable);
    }

    // -- Policy: output overflow ladder -------------------------------------

    #[test]
    fn output_overflow_first_action_is_reduce_budget() {
        let d = policy().next_action(&output_overflow_signal(), &RecoveryState::new());
        assert_eq!(d.action(), RecoveryAction::ReduceOutputBudget);
        assert_eq!(d.reason(), RecoveryReason::OutputBudgetReductionAvailable);
    }

    #[test]
    fn output_overflow_after_reduction_escalates_to_compaction() {
        let mut state = RecoveryState::new();
        state.record_output_reduction();
        let d = policy().next_action(&output_overflow_signal(), &state);
        assert_eq!(d.action(), RecoveryAction::CompactContext);
        assert_eq!(d.reason(), RecoveryReason::OutputReductionInsufficient);
    }

    // -- Policy: input overflow ladder --------------------------------------

    #[test]
    fn input_overflow_first_action_is_compact() {
        let d = policy().next_action(&input_overflow_signal(), &RecoveryState::new());
        assert_eq!(d.action(), RecoveryAction::CompactContext);
        assert_eq!(d.reason(), RecoveryReason::CompactionRequired);
    }

    #[test]
    fn input_overflow_skips_output_reduction() {
        let d = policy().next_action(&input_overflow_signal(), &RecoveryState::new());
        assert_ne!(d.action(), RecoveryAction::ReduceOutputBudget);
    }

    // -- Policy: compaction progress gating ---------------------------------

    #[test]
    fn first_compaction_with_progress_allows_second() {
        let mut state = RecoveryState::new();
        state.record_compaction(progress_with_freed(2_000));
        let d = policy().next_action(&input_overflow_signal(), &state);
        assert_eq!(d.action(), RecoveryAction::CompactContext);
        assert_eq!(d.reason(), RecoveryReason::CompactionProgressSufficient);
    }

    #[test]
    fn first_compaction_without_progress_aborts() {
        let mut state = RecoveryState::new();
        // Free only 100 tokens — below the fixed threshold of 500.
        state.record_compaction(progress_with_freed(100));
        let d = policy().next_action(&input_overflow_signal(), &state);
        assert_eq!(d.action(), RecoveryAction::Abort);
        assert_eq!(d.reason(), RecoveryReason::CompactionProgressInsufficient);
    }

    #[test]
    fn second_compaction_exhausts_attempts() {
        let mut state = RecoveryState::new();
        state.record_compaction(progress_with_freed(2_000));
        state.record_compaction(progress_with_freed(1_000));
        let d = policy().next_action(&input_overflow_signal(), &state);
        assert_eq!(d.action(), RecoveryAction::Abort);
        assert_eq!(d.reason(), RecoveryReason::CompactionAttemptsExhausted);
    }

    #[test]
    fn progress_at_exact_threshold_allows_second_compaction() {
        let mut state = RecoveryState::new();
        state.record_compaction(progress_with_freed(MIN_PROGRESS_TOKENS));
        let d = policy().next_action(&input_overflow_signal(), &state);
        assert_eq!(d.action(), RecoveryAction::CompactContext);
    }

    #[test]
    fn progress_one_below_threshold_does_not_allow_second_compaction() {
        let mut state = RecoveryState::new();
        state.record_compaction(progress_with_freed(MIN_PROGRESS_TOKENS - 1));
        let d = policy().next_action(&input_overflow_signal(), &state);
        assert_eq!(d.action(), RecoveryAction::Abort);
        assert_eq!(d.reason(), RecoveryReason::CompactionProgressInsufficient);
    }

    // -- Policy: test-only config knobs -------------------------------------

    #[test]
    fn custom_progress_threshold_is_respected() {
        let p = OverflowRecoveryPolicy::with_test_config(2, 5_000);
        let mut state = RecoveryState::new();
        // Free 2000 — above default 500 but below test threshold 5000.
        state.record_compaction(progress_with_freed(2_000));
        let d = p.next_action(&input_overflow_signal(), &state);
        assert_eq!(d.action(), RecoveryAction::Abort);
        assert_eq!(d.reason(), RecoveryReason::CompactionProgressInsufficient);
    }

    #[test]
    fn zero_max_compactions_aborts_before_any_compaction() {
        let p = OverflowRecoveryPolicy::with_test_config(0, MIN_PROGRESS_TOKENS);
        let d = p.next_action(&input_overflow_signal(), &RecoveryState::new());
        assert_eq!(d.action(), RecoveryAction::Abort);
        assert_eq!(d.reason(), RecoveryReason::CompactionAttemptsExhausted);
    }

    #[test]
    fn zero_max_compactions_still_allows_output_reduction() {
        let p = OverflowRecoveryPolicy::with_test_config(0, MIN_PROGRESS_TOKENS);
        let d = p.next_action(&output_overflow_signal(), &RecoveryState::new());
        assert_eq!(d.action(), RecoveryAction::ReduceOutputBudget);
    }

    // -- Policy: full episode traces ----------------------------------------

    #[test]
    fn full_output_overflow_episode_with_recovery() {
        let p = policy();
        let signal = output_overflow_signal();
        let mut state = RecoveryState::new();

        // Step 1: reduce output budget.
        let d1 = p.next_action(&signal, &state);
        assert_eq!(d1.action(), RecoveryAction::ReduceOutputBudget);
        state.record_output_reduction();

        // Step 2: still overflowing → first compaction.
        let d2 = p.next_action(&signal, &state);
        assert_eq!(d2.action(), RecoveryAction::CompactContext);
        assert_eq!(d2.reason(), RecoveryReason::OutputReductionInsufficient);
        state.record_compaction(progress_with_freed(3_000));

        // Step 3: still overflowing → second compaction (progress OK).
        let d3 = p.next_action(&signal, &state);
        assert_eq!(d3.action(), RecoveryAction::CompactContext);
        assert_eq!(d3.reason(), RecoveryReason::CompactionProgressSufficient);
        state.record_compaction(progress_with_freed(1_000));

        // Step 4: still overflowing → exhausted.
        let d4 = p.next_action(&signal, &state);
        assert_eq!(d4.action(), RecoveryAction::Abort);
        assert_eq!(d4.reason(), RecoveryReason::CompactionAttemptsExhausted);
        assert_eq!(state.total_attempts(), 3);
    }

    #[test]
    fn full_input_overflow_episode_insufficient_progress() {
        let p = policy();
        let signal = input_overflow_signal();
        let mut state = RecoveryState::new();

        // Step 1: first compaction.
        let d1 = p.next_action(&signal, &state);
        assert_eq!(d1.action(), RecoveryAction::CompactContext);
        assert_eq!(d1.reason(), RecoveryReason::CompactionRequired);
        // Compaction barely freed anything.
        state.record_compaction(progress_with_freed(50));

        // Step 2: insufficient progress → abort.
        let d2 = p.next_action(&signal, &state);
        assert_eq!(d2.action(), RecoveryAction::Abort);
        assert_eq!(d2.reason(), RecoveryReason::CompactionProgressInsufficient);
        assert_eq!(state.total_attempts(), 1);
    }

    // -- RecoveryEvent constructability -------------------------------------

    #[test]
    fn recovery_events_are_constructable_with_correct_types() {
        let _started = RecoveryEvent::EpisodeStarted {
            overflow_kind: OverflowKind::InputOverflow,
        };
        let _decided = RecoveryEvent::ActionDecided {
            decision: RecoveryDecision::from_reason(RecoveryReason::CompactionRequired),
            attempt_index: 0,
        };
        let _resolved = RecoveryEvent::EpisodeResolved { total_attempts: 1 };
        // EpisodeExhausted only accepts AbortReason — not RecoveryReason.
        let _exhausted = RecoveryEvent::EpisodeExhausted {
            reason: AbortReason::CompactionAttemptsExhausted,
            total_attempts: 2,
        };
    }

    #[test]
    fn episode_exhausted_reason_converts_to_recovery_reason() {
        let abort = AbortReason::CompactionProgressInsufficient;
        let reason: RecoveryReason = abort.into();
        assert_eq!(reason, RecoveryReason::CompactionProgressInsufficient);
    }
}
