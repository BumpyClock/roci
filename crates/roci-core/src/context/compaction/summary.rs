//! Summary compaction assembly.
//!
//! Provider-agnostic logic for assembling a [`SummaryCompactionResult`] from
//! a prepared compaction split, a formatted summary message, and
//! file-operation metadata. This module does not invoke any LLM; the caller
//! is responsible for obtaining the summary text and formatting it into
//! the desired visible summary message.

use crate::context::tokens::estimate_message_tokens;
use crate::types::ModelMessage;

use super::types::{
    CompactionSpan, CompactionSuffix, FileOperationSnapshot, PreparedCompaction, SummaryArtifact,
    SummaryCompactionResult,
};

/// Assemble a [`SummaryCompactionResult`] from its components.
///
/// This is the core, provider-agnostic compaction assembly. It builds the
/// compacted message list, computes token/entry metrics, and updates the
/// suffix state for the next compaction round.
///
/// The assembled message order is:
/// `system_prefix ++ [summary_message] ++ turn_prefix ++ kept`
///
/// # Arguments
///
/// * `system_prefix` — System messages preceding the conversation.
/// * `prepared` — The compaction split (to-summarize, turn-prefix, kept).
/// * `summary_text` — Raw summary text stored in the [`SummaryArtifact`].
/// * `summary_message` — Formatted message inserted into the conversation.
///   The caller wraps `summary_text` in whatever visible format the runtime
///   requires (e.g. `<compaction_summary>…</compaction_summary>`).
/// * `file_ops` — File-operation snapshot from the summarized span.
/// * `suffix` — Suffix state carried from previous compaction rounds.
pub fn assemble_summary_compaction(
    system_prefix: &[ModelMessage],
    prepared: &PreparedCompaction,
    summary_text: String,
    summary_message: ModelMessage,
    file_ops: FileOperationSnapshot,
    suffix: CompactionSuffix,
) -> SummaryCompactionResult {
    let span = CompactionSpan::from_prepared(prepared, estimate_message_tokens);

    let tokens_before: usize = system_prefix
        .iter()
        .chain(prepared.messages_to_summarize.iter())
        .chain(prepared.turn_prefix_messages.iter())
        .chain(prepared.kept_messages.iter())
        .map(estimate_message_tokens)
        .sum();

    let mut messages = Vec::with_capacity(
        system_prefix.len()
            + 1
            + prepared.turn_prefix_messages.len()
            + prepared.kept_messages.len(),
    );
    messages.extend_from_slice(system_prefix);
    messages.push(summary_message);
    messages.extend_from_slice(&prepared.turn_prefix_messages);
    messages.extend_from_slice(&prepared.kept_messages);

    let tokens_after: usize = messages.iter().map(estimate_message_tokens).sum();
    let entries_removed = prepared.messages_to_summarize.len();

    let mut updated_suffix = suffix;
    updated_suffix.record_round(&file_ops);

    let artifact = SummaryArtifact {
        text: summary_text,
        file_operations: file_ops,
        span,
    };

    SummaryCompactionResult {
        messages,
        artifact,
        tokens_before,
        tokens_after,
        entries_removed,
        suffix: updated_suffix,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn make_system_prefix() -> Vec<ModelMessage> {
        vec![ModelMessage::system("You are helpful")]
    }

    fn make_prepared_no_split() -> PreparedCompaction {
        PreparedCompaction {
            messages_to_summarize: vec![
                ModelMessage::user("old question"),
                ModelMessage::assistant("old answer"),
            ],
            turn_prefix_messages: vec![],
            kept_messages: vec![
                ModelMessage::user("recent question"),
                ModelMessage::assistant("recent answer"),
            ],
            split_turn: false,
            cut_index: 2,
        }
    }

    fn make_prepared_with_split() -> PreparedCompaction {
        PreparedCompaction {
            messages_to_summarize: vec![ModelMessage::user("old question")],
            turn_prefix_messages: vec![
                ModelMessage::user("mid question"),
                ModelMessage::assistant("mid partial"),
            ],
            kept_messages: vec![ModelMessage::assistant("recent answer")],
            split_turn: true,
            cut_index: 3,
        }
    }

    fn sample_file_ops() -> FileOperationSnapshot {
        FileOperationSnapshot {
            read_files: BTreeSet::from(["src/main.rs".to_string()]),
            modified_files: BTreeSet::from(["src/lib.rs".to_string()]),
        }
    }

    #[test]
    fn assemble_preserves_system_prefix_and_kept_messages() {
        let result = assemble_summary_compaction(
            &make_system_prefix(),
            &make_prepared_no_split(),
            "summary".to_string(),
            ModelMessage::user("summary"),
            FileOperationSnapshot::default(),
            CompactionSuffix::default(),
        );

        assert_eq!(result.messages.len(), 4);
        assert_eq!(result.messages[0].text(), "You are helpful");
        assert_eq!(result.messages[1].text(), "summary");
        assert_eq!(result.messages[2].text(), "recent question");
        assert_eq!(result.messages[3].text(), "recent answer");
    }

    #[test]
    fn assemble_includes_turn_prefix_on_split() {
        let result = assemble_summary_compaction(
            &make_system_prefix(),
            &make_prepared_with_split(),
            "summary".to_string(),
            ModelMessage::user("summary"),
            FileOperationSnapshot::default(),
            CompactionSuffix::default(),
        );

        assert_eq!(result.messages.len(), 5);
        assert_eq!(result.messages[0].text(), "You are helpful");
        assert_eq!(result.messages[1].text(), "summary");
        assert_eq!(result.messages[2].text(), "mid question");
        assert_eq!(result.messages[3].text(), "mid partial");
        assert_eq!(result.messages[4].text(), "recent answer");
    }

    #[test]
    fn assemble_builds_correct_span_metadata() {
        let result = assemble_summary_compaction(
            &[],
            &make_prepared_no_split(),
            "summary".to_string(),
            ModelMessage::user("summary"),
            FileOperationSnapshot::default(),
            CompactionSuffix::default(),
        );

        assert_eq!(result.artifact.span.start_index, 0);
        assert_eq!(result.artifact.span.end_index, 2);
        assert_eq!(result.artifact.span.entries_compacted, 2);
        assert!(result.artifact.span.tokens_before > 0);
    }

    #[test]
    fn assemble_span_excludes_preserved_turn_prefix() {
        let result = assemble_summary_compaction(
            &[],
            &make_prepared_with_split(),
            "summary".to_string(),
            ModelMessage::user("summary"),
            FileOperationSnapshot::default(),
            CompactionSuffix::default(),
        );

        // Only messages_to_summarize(1) is in the span; turn_prefix(2) is preserved verbatim
        assert_eq!(result.artifact.span.entries_compacted, 1);
        assert_eq!(result.artifact.span.end_index, 1);
    }

    #[test]
    fn assemble_updates_suffix_with_new_round() {
        let initial_suffix = CompactionSuffix {
            compaction_count: 2,
            cumulative_file_ops: FileOperationSnapshot {
                read_files: BTreeSet::from(["old.rs".to_string()]),
                modified_files: BTreeSet::new(),
            },
        };

        let result = assemble_summary_compaction(
            &[],
            &make_prepared_no_split(),
            "summary".to_string(),
            ModelMessage::user("summary"),
            sample_file_ops(),
            initial_suffix,
        );

        assert_eq!(result.suffix.compaction_count, 3);
        assert!(result
            .suffix
            .cumulative_file_ops
            .read_files
            .contains("old.rs"));
        assert!(result
            .suffix
            .cumulative_file_ops
            .read_files
            .contains("src/main.rs"));
        assert!(result
            .suffix
            .cumulative_file_ops
            .modified_files
            .contains("src/lib.rs"));
    }

    #[test]
    fn assemble_records_entries_removed_count() {
        let result = assemble_summary_compaction(
            &[],
            &make_prepared_no_split(),
            "summary".to_string(),
            ModelMessage::user("summary"),
            FileOperationSnapshot::default(),
            CompactionSuffix::default(),
        );

        assert_eq!(result.entries_removed, 2);
    }

    #[test]
    fn assemble_computes_positive_token_counts() {
        let result = assemble_summary_compaction(
            &make_system_prefix(),
            &make_prepared_no_split(),
            "summary".to_string(),
            ModelMessage::user("summary"),
            FileOperationSnapshot::default(),
            CompactionSuffix::default(),
        );

        assert!(result.tokens_before > 0, "tokens_before should be positive");
        assert!(result.tokens_after > 0, "tokens_after should be positive");
    }

    #[test]
    fn assemble_artifact_carries_text_and_file_ops() {
        let file_ops = sample_file_ops();

        let result = assemble_summary_compaction(
            &[],
            &make_prepared_no_split(),
            "the raw summary".to_string(),
            ModelMessage::user("formatted summary"),
            file_ops.clone(),
            CompactionSuffix::default(),
        );

        assert_eq!(result.artifact.text, "the raw summary");
        assert_eq!(result.artifact.file_operations, file_ops);
    }

    #[test]
    fn assemble_with_empty_system_prefix() {
        let result = assemble_summary_compaction(
            &[],
            &make_prepared_no_split(),
            "summary".to_string(),
            ModelMessage::user("summary"),
            FileOperationSnapshot::default(),
            CompactionSuffix::default(),
        );

        assert_eq!(result.messages.len(), 3);
        assert_eq!(result.messages[0].text(), "summary");
        assert_eq!(result.messages[1].text(), "recent question");
    }
}
