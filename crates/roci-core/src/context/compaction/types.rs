//! Types used across compaction operations.
//!
//! This module defines the public SDK-facing compaction contract:
//! strategy selection, request/result envelopes, summary artifacts,
//! and span/suffix metadata carried across successive compaction rounds.

use std::collections::BTreeSet;

use crate::types::ModelMessage;

// ---------------------------------------------------------------------------
// Existing types (Wave 0/1)
// ---------------------------------------------------------------------------

/// Inclusive index range identifying a branch segment within a message list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchEntryRange {
    pub start_entry_index: usize,
    pub end_entry_index: usize,
}

/// Output of [`super::helpers::prepare_compaction`].
///
/// Splits a message list into the portion to summarize, an optional
/// turn-prefix (when the cut lands mid-turn), and the kept tail.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PreparedCompaction {
    pub messages_to_summarize: Vec<ModelMessage>,
    pub turn_prefix_messages: Vec<ModelMessage>,
    pub kept_messages: Vec<ModelMessage>,
    pub split_turn: bool,
    pub cut_index: usize,
}

// ---------------------------------------------------------------------------
// Wave 2 — Compaction contract layer
// ---------------------------------------------------------------------------

/// Strategy for compacting a conversation's context window.
///
/// Each variant maps to a distinct compaction algorithm. `Micro` operates
/// purely on message content (no LLM call), while `Summary` invokes a
/// model to generate a condensed replacement for older messages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CompactionStrategy {
    /// Fine-grained within-message compaction: tool-result truncation,
    /// image elision, thinking-block removal. No LLM call required.
    Micro,
    /// LLM-generated summary replaces older messages.
    #[default]
    Summary,
}

impl std::fmt::Display for CompactionStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Micro => f.write_str("micro"),
            Self::Summary => f.write_str("summary"),
        }
    }
}

/// Snapshot of files read and modified across a compaction span.
///
/// This is the public SDK counterpart of the agent-internal
/// `FileOperationSet`. It captures cumulative file-access metadata
/// so that summary prompts and hooks can reference which files were
/// touched in the compacted history.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileOperationSnapshot {
    /// Paths that were read (e.g. via `read_file`, `view`).
    pub read_files: BTreeSet<String>,
    /// Paths that were created, modified, or deleted.
    pub modified_files: BTreeSet<String>,
}

impl FileOperationSnapshot {
    /// Merge another snapshot into this one, accumulating both sets.
    pub fn merge(&mut self, other: &FileOperationSnapshot) {
        self.read_files.extend(other.read_files.iter().cloned());
        self.modified_files
            .extend(other.modified_files.iter().cloned());
    }

    /// Returns `true` when both file sets are empty.
    pub fn is_empty(&self) -> bool {
        self.read_files.is_empty() && self.modified_files.is_empty()
    }
}

/// Metadata describing the message range consumed by a compaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionSpan {
    /// Index of the first message included in the compacted range.
    pub start_index: usize,
    /// Index one past the last message in the compacted range
    /// (i.e. `messages[start_index..end_index]` was compacted).
    pub end_index: usize,
    /// Number of messages consumed by the compaction.
    pub entries_compacted: usize,
    /// Estimated token count of the compacted range before compaction.
    pub tokens_before: usize,
}

impl CompactionSpan {
    /// Build a span from a [`PreparedCompaction`].
    ///
    /// The span covers only `messages_to_summarize` — the messages
    /// actually replaced by the summary. `turn_prefix_messages` are
    /// preserved verbatim in the output and are not part of the span.
    pub fn from_prepared(
        prepared: &PreparedCompaction,
        token_estimator: fn(&ModelMessage) -> usize,
    ) -> Self {
        let entries = prepared.messages_to_summarize.len();
        let tokens: usize = prepared
            .messages_to_summarize
            .iter()
            .map(token_estimator)
            .sum();

        Self {
            start_index: 0,
            end_index: entries,
            entries_compacted: entries,
            tokens_before: tokens,
        }
    }
}

/// Suffix metadata carried forward across successive compactions.
///
/// Each compaction round updates this state so the next round knows
/// the conversation's compaction history without re-scanning messages.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompactionSuffix {
    /// Running count of compactions applied to this conversation.
    pub compaction_count: u32,
    /// Cumulative file operations across all compaction rounds.
    pub cumulative_file_ops: FileOperationSnapshot,
}

impl CompactionSuffix {
    /// Record a completed compaction round, incrementing the count and
    /// merging file operations from the latest artifact.
    pub fn record_round(&mut self, file_ops: &FileOperationSnapshot) {
        self.compaction_count += 1;
        self.cumulative_file_ops.merge(file_ops);
    }
}

/// Output artifact from an LLM-generated summary compaction.
///
/// Contains the generated summary text, the file-operation snapshot
/// extracted from the summarized span, and positional metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryArtifact {
    /// The generated summary text (ready to wrap in a summary message).
    pub text: String,
    /// File operations captured from the summarized span.
    pub file_operations: FileOperationSnapshot,
    /// Positional metadata about the range that was summarized.
    pub span: CompactionSpan,
}

// ---------------------------------------------------------------------------
// Strategy-typed request
// ---------------------------------------------------------------------------

/// Payload for a [`CompactionRequest::Micro`] compaction.
///
/// Micro compaction operates purely on message content (tool-result
/// truncation, image elision, thinking-block removal) — no LLM call.
#[derive(Debug, Clone, PartialEq)]
pub struct MicroCompactionRequest {
    /// The prepared split (to-compact / turn-prefix / kept).
    pub prepared: PreparedCompaction,
    /// Suffix state carried from previous compaction rounds.
    pub suffix: CompactionSuffix,
}

/// Payload for a [`CompactionRequest::Summary`] compaction.
///
/// Summary compaction invokes a model to generate a condensed replacement
/// for older messages.
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryCompactionRequest {
    /// The prepared split (to-summarize / turn-prefix / kept).
    pub prepared: PreparedCompaction,
    /// Token budget reserved for the summary model's output.
    pub reserve_tokens: usize,
    /// Optional model identifier override for summary generation.
    pub model_id: Option<String>,
    /// Suffix state carried from previous compaction rounds.
    pub suffix: CompactionSuffix,
}

/// A fully specified, strategy-typed compaction request.
///
/// Each variant carries only the fields valid for that strategy,
/// making it impossible to construct an invalid combination
/// (e.g. a micro request with `model_id`).
#[derive(Debug, Clone, PartialEq)]
pub enum CompactionRequest {
    /// Fine-grained within-message compaction — no LLM call.
    Micro(MicroCompactionRequest),
    /// LLM-generated summary replaces older messages.
    Summary(SummaryCompactionRequest),
}

impl CompactionRequest {
    /// The [`CompactionStrategy`] encoded by this request.
    pub fn strategy(&self) -> CompactionStrategy {
        match self {
            Self::Micro(_) => CompactionStrategy::Micro,
            Self::Summary(_) => CompactionStrategy::Summary,
        }
    }

    /// The prepared message split, regardless of strategy.
    pub fn prepared(&self) -> &PreparedCompaction {
        match self {
            Self::Micro(r) => &r.prepared,
            Self::Summary(r) => &r.prepared,
        }
    }

    /// Suffix state carried from previous compaction rounds.
    pub fn suffix(&self) -> &CompactionSuffix {
        match self {
            Self::Micro(r) => &r.suffix,
            Self::Summary(r) => &r.suffix,
        }
    }
}

// ---------------------------------------------------------------------------
// Strategy-typed result
// ---------------------------------------------------------------------------

/// Result payload for a [`CompactionResult::Micro`] compaction.
///
/// Micro compaction never produces a summary artifact.
#[derive(Debug, Clone, PartialEq)]
pub struct MicroCompactionResult {
    /// The compacted message list, ready to replace the original.
    pub messages: Vec<ModelMessage>,
    /// Estimated total tokens before compaction.
    pub tokens_before: usize,
    /// Estimated total tokens after compaction.
    pub tokens_after: usize,
    /// Number of messages removed from the context.
    pub entries_removed: usize,
    /// Updated suffix metadata for the next compaction round.
    pub suffix: CompactionSuffix,
}

/// Result payload for a [`CompactionResult::Summary`] compaction.
///
/// Summary compaction always produces a [`SummaryArtifact`].
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryCompactionResult {
    /// The compacted message list, ready to replace the original.
    pub messages: Vec<ModelMessage>,
    /// The summary artifact (always present for summary compaction).
    pub artifact: SummaryArtifact,
    /// Estimated total tokens before compaction.
    pub tokens_before: usize,
    /// Estimated total tokens after compaction.
    pub tokens_after: usize,
    /// Number of messages removed from the context.
    pub entries_removed: usize,
    /// Updated suffix metadata for the next compaction round.
    pub suffix: CompactionSuffix,
}

/// Result of a compaction operation, typed by strategy.
///
/// Each variant carries only the fields valid for that strategy.
/// Summary results always include a [`SummaryArtifact`]; micro
/// results never do.
#[derive(Debug, Clone, PartialEq)]
pub enum CompactionResult {
    /// Result of a micro compaction (no summary artifact).
    Micro(MicroCompactionResult),
    /// Result of a summary compaction (artifact required).
    Summary(SummaryCompactionResult),
}

impl CompactionResult {
    /// The [`CompactionStrategy`] that produced this result.
    pub fn strategy(&self) -> CompactionStrategy {
        match self {
            Self::Micro(_) => CompactionStrategy::Micro,
            Self::Summary(_) => CompactionStrategy::Summary,
        }
    }

    /// The compacted message list, regardless of strategy.
    pub fn messages(&self) -> &[ModelMessage] {
        match self {
            Self::Micro(r) => &r.messages,
            Self::Summary(r) => &r.messages,
        }
    }

    /// Estimated total tokens before compaction.
    pub fn tokens_before(&self) -> usize {
        match self {
            Self::Micro(r) => r.tokens_before,
            Self::Summary(r) => r.tokens_before,
        }
    }

    /// Estimated total tokens after compaction.
    pub fn tokens_after(&self) -> usize {
        match self {
            Self::Micro(r) => r.tokens_after,
            Self::Summary(r) => r.tokens_after,
        }
    }

    /// Number of messages removed from the context.
    pub fn entries_removed(&self) -> usize {
        match self {
            Self::Micro(r) => r.entries_removed,
            Self::Summary(r) => r.entries_removed,
        }
    }

    /// Updated suffix metadata for the next compaction round.
    pub fn suffix(&self) -> &CompactionSuffix {
        match self {
            Self::Micro(r) => &r.suffix,
            Self::Summary(r) => &r.suffix,
        }
    }

    /// The summary artifact, if this is a summary result.
    pub fn artifact(&self) -> Option<&SummaryArtifact> {
        match self {
            Self::Micro(_) => None,
            Self::Summary(r) => Some(&r.artifact),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::tokens::estimate_message_tokens;

    // -- CompactionStrategy ---------------------------------------------------

    #[test]
    fn strategy_default_is_summary() {
        assert_eq!(CompactionStrategy::default(), CompactionStrategy::Summary);
    }

    #[test]
    fn strategy_display_formats_lowercase() {
        assert_eq!(CompactionStrategy::Micro.to_string(), "micro");
        assert_eq!(CompactionStrategy::Summary.to_string(), "summary");
    }

    #[test]
    fn strategy_equality_and_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(CompactionStrategy::Micro);
        set.insert(CompactionStrategy::Summary);
        assert_eq!(set.len(), 2);
        // Duplicate insert should not increase size.
        set.insert(CompactionStrategy::Micro);
        assert_eq!(set.len(), 2);
    }

    // -- FileOperationSnapshot -----------------------------------------------

    #[test]
    fn file_op_snapshot_default_is_empty() {
        let snap = FileOperationSnapshot::default();
        assert!(snap.is_empty());
        assert!(snap.read_files.is_empty());
        assert!(snap.modified_files.is_empty());
    }

    #[test]
    fn file_op_snapshot_merge_accumulates_both_sets() {
        let mut a = FileOperationSnapshot {
            read_files: BTreeSet::from(["src/a.rs".to_string()]),
            modified_files: BTreeSet::from(["src/b.rs".to_string()]),
        };
        let b = FileOperationSnapshot {
            read_files: BTreeSet::from(["src/c.rs".to_string()]),
            modified_files: BTreeSet::from(["src/b.rs".to_string(), "src/d.rs".to_string()]),
        };

        a.merge(&b);

        assert_eq!(a.read_files.len(), 2);
        assert!(a.read_files.contains("src/a.rs"));
        assert!(a.read_files.contains("src/c.rs"));
        assert_eq!(a.modified_files.len(), 2);
        assert!(a.modified_files.contains("src/b.rs"));
        assert!(a.modified_files.contains("src/d.rs"));
    }

    #[test]
    fn file_op_snapshot_is_empty_false_when_populated() {
        let snap = FileOperationSnapshot {
            read_files: BTreeSet::from(["f.rs".to_string()]),
            modified_files: BTreeSet::new(),
        };
        assert!(!snap.is_empty());
    }

    // -- CompactionSpan -------------------------------------------------------

    #[test]
    fn compaction_span_from_prepared_covers_only_summarized_messages() {
        let prepared = PreparedCompaction {
            messages_to_summarize: vec![ModelMessage::system("sys"), ModelMessage::user("u1")],
            turn_prefix_messages: vec![ModelMessage::assistant("a1")],
            kept_messages: vec![ModelMessage::user("u2")],
            split_turn: true,
            cut_index: 3,
        };

        let span = CompactionSpan::from_prepared(&prepared, estimate_message_tokens);

        assert_eq!(span.start_index, 0);
        assert_eq!(span.end_index, 2);
        assert_eq!(span.entries_compacted, 2);
        assert!(span.tokens_before > 0);
    }

    #[test]
    fn compaction_span_from_prepared_empty_prefix() {
        let prepared = PreparedCompaction {
            messages_to_summarize: vec![ModelMessage::user("only")],
            turn_prefix_messages: vec![],
            kept_messages: vec![ModelMessage::assistant("kept")],
            split_turn: false,
            cut_index: 1,
        };

        let span = CompactionSpan::from_prepared(&prepared, estimate_message_tokens);

        assert_eq!(span.entries_compacted, 1);
        assert_eq!(span.end_index, 1);
    }

    // -- CompactionSuffix -----------------------------------------------------

    #[test]
    fn suffix_default_starts_at_zero() {
        let suffix = CompactionSuffix::default();
        assert_eq!(suffix.compaction_count, 0);
        assert!(suffix.cumulative_file_ops.is_empty());
    }

    #[test]
    fn suffix_record_round_increments_count_and_merges_ops() {
        let mut suffix = CompactionSuffix::default();

        let ops1 = FileOperationSnapshot {
            read_files: BTreeSet::from(["a.rs".to_string()]),
            modified_files: BTreeSet::new(),
        };
        suffix.record_round(&ops1);
        assert_eq!(suffix.compaction_count, 1);
        assert!(suffix.cumulative_file_ops.read_files.contains("a.rs"));

        let ops2 = FileOperationSnapshot {
            read_files: BTreeSet::new(),
            modified_files: BTreeSet::from(["b.rs".to_string()]),
        };
        suffix.record_round(&ops2);
        assert_eq!(suffix.compaction_count, 2);
        assert!(suffix.cumulative_file_ops.read_files.contains("a.rs"));
        assert!(suffix.cumulative_file_ops.modified_files.contains("b.rs"));
    }

    // -- CompactionRequest (strategy-typed) -----------------------------------

    #[test]
    fn micro_request_has_no_model_or_reserve_fields() {
        let request = CompactionRequest::Micro(MicroCompactionRequest {
            prepared: PreparedCompaction::default(),
            suffix: CompactionSuffix::default(),
        });

        assert_eq!(request.strategy(), CompactionStrategy::Micro);
        assert_eq!(request.prepared(), &PreparedCompaction::default());
        assert_eq!(request.suffix(), &CompactionSuffix::default());
    }

    #[test]
    fn summary_request_carries_model_and_reserve_tokens() {
        let request = CompactionRequest::Summary(SummaryCompactionRequest {
            prepared: PreparedCompaction::default(),
            reserve_tokens: 8_192,
            model_id: Some("test-model".to_string()),
            suffix: CompactionSuffix::default(),
        });

        assert_eq!(request.strategy(), CompactionStrategy::Summary);

        match &request {
            CompactionRequest::Summary(r) => {
                assert_eq!(r.reserve_tokens, 8_192);
                assert_eq!(r.model_id.as_deref(), Some("test-model"));
            }
            _ => panic!("expected Summary variant"),
        }
    }

    // -- SummaryArtifact ------------------------------------------------------

    #[test]
    fn summary_artifact_holds_text_and_span() {
        let artifact = SummaryArtifact {
            text: "Implemented auth module".to_string(),
            file_operations: FileOperationSnapshot {
                read_files: BTreeSet::from(["src/auth.rs".to_string()]),
                modified_files: BTreeSet::from(["src/auth.rs".to_string()]),
            },
            span: CompactionSpan {
                start_index: 0,
                end_index: 5,
                entries_compacted: 5,
                tokens_before: 2_000,
            },
        };

        assert_eq!(artifact.text, "Implemented auth module");
        assert_eq!(artifact.span.entries_compacted, 5);
        assert!(!artifact.file_operations.is_empty());
    }

    // -- CompactionResult (strategy-typed) ------------------------------------

    #[test]
    fn summary_result_always_carries_artifact() {
        let artifact = SummaryArtifact {
            text: "summary text".to_string(),
            file_operations: FileOperationSnapshot::default(),
            span: CompactionSpan {
                start_index: 0,
                end_index: 3,
                entries_compacted: 3,
                tokens_before: 1_500,
            },
        };

        let result = CompactionResult::Summary(SummaryCompactionResult {
            messages: vec![ModelMessage::user("kept")],
            artifact,
            tokens_before: 5_000,
            tokens_after: 1_000,
            entries_removed: 3,
            suffix: CompactionSuffix {
                compaction_count: 1,
                cumulative_file_ops: FileOperationSnapshot::default(),
            },
        });

        assert_eq!(result.strategy(), CompactionStrategy::Summary);
        assert!(result.artifact().is_some());
        assert_eq!(result.entries_removed(), 3);
        assert_eq!(result.tokens_before() - result.tokens_after(), 4_000);
    }

    #[test]
    fn micro_result_never_has_artifact() {
        let result = CompactionResult::Micro(MicroCompactionResult {
            messages: vec![ModelMessage::user("kept")],
            tokens_before: 3_000,
            tokens_after: 2_500,
            entries_removed: 0,
            suffix: CompactionSuffix::default(),
        });

        assert_eq!(result.strategy(), CompactionStrategy::Micro);
        assert!(result.artifact().is_none());
    }

    #[test]
    fn result_accessors_work_across_variants() {
        let micro = CompactionResult::Micro(MicroCompactionResult {
            messages: vec![ModelMessage::user("m1"), ModelMessage::assistant("m2")],
            tokens_before: 1_000,
            tokens_after: 800,
            entries_removed: 1,
            suffix: CompactionSuffix {
                compaction_count: 2,
                cumulative_file_ops: FileOperationSnapshot::default(),
            },
        });

        let summary = CompactionResult::Summary(SummaryCompactionResult {
            messages: vec![ModelMessage::user("s1")],
            artifact: SummaryArtifact {
                text: "summary".to_string(),
                file_operations: FileOperationSnapshot::default(),
                span: CompactionSpan {
                    start_index: 0,
                    end_index: 4,
                    entries_compacted: 4,
                    tokens_before: 2_000,
                },
            },
            tokens_before: 2_000,
            tokens_after: 500,
            entries_removed: 4,
            suffix: CompactionSuffix::default(),
        });

        // Uniform accessors
        assert_eq!(micro.messages().len(), 2);
        assert_eq!(summary.messages().len(), 1);
        assert_eq!(micro.suffix().compaction_count, 2);
        assert_eq!(summary.suffix().compaction_count, 0);
    }
}
