//! Provider-agnostic context-window management.
//!
//! This module owns token estimation, budget selection, overflow policy types,
//! recovery policy, and compaction-preparation helpers. It intentionally
//! contains no agent-internal summary formats or provider-specific tokenizers.
//!
//! # Module layout
//!
//! - [`tokens`] — heuristic token counting, `TokenCounter` trait, typed `TokenCount`
//! - [`budget`] — token budget configuration, snapshots, and decision types
//! - [`overflow`] — overflow detection and classification contracts
//! - [`recovery`] — overflow recovery policy, decision types, and event contracts
//! - [`compaction`] — compaction-preparation helpers and types

pub mod budget;
pub mod compaction;
pub mod overflow;
pub mod recovery;
pub mod tokens;

pub use self::budget::{
    select_messages_with_token_budget_newest_first, BudgetDecision, BudgetSnapshot, ContextBudget,
};
pub use self::compaction::{
    assemble_summary_compaction, collect_entries_between_branches, compact_micro,
    compact_micro_with_config, find_compaction_cut_index, prepare_compaction, BranchEntryRange,
    CompactionRequest, CompactionResult, CompactionSpan, CompactionStrategy, CompactionSuffix,
    FileOperationSnapshot, MicroCompactionConfig, MicroCompactionRequest, MicroCompactionResult,
    PreparedCompaction, SummaryArtifact, SummaryCompactionRequest, SummaryCompactionResult,
};
pub use self::recovery::{
    AbortReason, CompactionProgress, OverflowRecoveryPolicy, RecoveryAction, RecoveryDecision,
    RecoveryEvent, RecoveryReason, RecoveryState,
};
pub use self::tokens::{
    estimate_context_usage, estimate_message_tokens, estimate_text_tokens, ContextUsage,
    ContextUsageSnapshot, CountAccuracy, HeuristicTokenCounter, SnapshotConfidence, SnapshotSource,
    TokenCount, TokenCountSource, TokenCounter,
};
