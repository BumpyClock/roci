//! Reusable compaction-preparation helpers.
//!
//! These operate on raw `ModelMessage` slices and have no dependency on
//! agent-internal summary formats or file-operation extraction.

use crate::types::{ModelMessage, Role};

use super::types::{BranchEntryRange, PreparedCompaction};
use crate::context::tokens::estimate_message_tokens;

/// Find the index at which to cut messages for compaction.
///
/// Walks backward from the end, accumulating token estimates until
/// `keep_recent_tokens` is exceeded. The cut is then nudged forward past any
/// leading `Tool` result so the kept tail always starts at a non-tool message.
pub fn find_compaction_cut_index(messages: &[ModelMessage], keep_recent_tokens: usize) -> usize {
    if messages.is_empty() {
        return 0;
    }

    if keep_recent_tokens == 0 {
        return messages.len();
    }

    let mut kept_tokens = 0usize;
    let mut cut_index = messages.len();

    for idx in (0..messages.len()).rev() {
        kept_tokens += estimate_message_tokens(&messages[idx]);
        cut_index = idx;
        if kept_tokens > keep_recent_tokens {
            cut_index = (idx + 1).min(messages.len());
            break;
        }
    }

    while cut_index < messages.len() && messages[cut_index].role == Role::Tool {
        cut_index += 1;
    }

    cut_index.min(messages.len())
}

/// Prepare a compaction split from a message list.
///
/// Returns a [`PreparedCompaction`] that separates the messages into a
/// to-summarize head, an optional turn-prefix (when the cut lands inside a
/// user turn), and a kept tail.
pub fn prepare_compaction(
    messages: &[ModelMessage],
    keep_recent_tokens: usize,
) -> PreparedCompaction {
    let cut_index = find_compaction_cut_index(messages, keep_recent_tokens);

    if cut_index == 0 || cut_index >= messages.len() {
        return PreparedCompaction {
            messages_to_summarize: messages[..cut_index].to_vec(),
            turn_prefix_messages: Vec::new(),
            kept_messages: messages[cut_index..].to_vec(),
            split_turn: false,
            cut_index,
        };
    }

    let turn_start = messages[..cut_index]
        .iter()
        .rposition(|m| m.role == Role::User)
        .unwrap_or(cut_index);

    let split_turn = turn_start < cut_index;

    if split_turn {
        PreparedCompaction {
            messages_to_summarize: messages[..turn_start].to_vec(),
            turn_prefix_messages: messages[turn_start..cut_index].to_vec(),
            kept_messages: messages[cut_index..].to_vec(),
            split_turn: true,
            cut_index,
        }
    } else {
        PreparedCompaction {
            messages_to_summarize: messages[..cut_index].to_vec(),
            turn_prefix_messages: Vec::new(),
            kept_messages: messages[cut_index..].to_vec(),
            split_turn: false,
            cut_index,
        }
    }
}

/// Collect messages in the inclusive range `[start, end]` between branch points.
///
/// Handles reversed or out-of-bounds indices gracefully.
pub fn collect_entries_between_branches(
    messages: &[ModelMessage],
    entry_range: BranchEntryRange,
) -> Vec<ModelMessage> {
    if messages.is_empty() {
        return Vec::new();
    }

    let start = entry_range
        .start_entry_index
        .min(entry_range.end_entry_index);
    if start >= messages.len() {
        return Vec::new();
    }

    let end = entry_range
        .start_entry_index
        .max(entry_range.end_entry_index)
        .min(messages.len() - 1);
    messages[start..=end].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::tokens::estimate_message_tokens;
    use crate::types::{AgentToolCall, ContentPart, ModelMessage, Role};

    fn assistant_with_tool_call(name: &str, arguments: serde_json::Value) -> ModelMessage {
        ModelMessage {
            role: Role::Assistant,
            content: vec![ContentPart::ToolCall(AgentToolCall {
                id: "call_1".to_string(),
                name: name.to_string(),
                arguments,
                recipient: None,
            })],
            name: None,
            timestamp: None,
        }
    }

    #[test]
    fn find_compaction_cut_index_never_starts_at_tool_result() {
        let messages = vec![
            ModelMessage::user("u1"),
            assistant_with_tool_call("read_file", serde_json::json!({"path": "/tmp/a.txt"})),
            ModelMessage::tool_result("call_1", serde_json::json!({"ok": true}), false),
            ModelMessage::assistant("done"),
        ];

        let keep_recent_tokens =
            estimate_message_tokens(messages.last().expect("has last message"));
        let cut_index = find_compaction_cut_index(&messages, keep_recent_tokens);

        assert!(cut_index < messages.len());
        assert_ne!(messages[cut_index].role, Role::Tool);
    }

    #[test]
    fn prepare_compaction_splits_turn_prefix_when_cut_inside_turn() {
        let messages = vec![
            ModelMessage::system("You are helpful"),
            ModelMessage::user("request"),
            ModelMessage::assistant("starting"),
            assistant_with_tool_call("read_file", serde_json::json!({"path": "/tmp/a.txt"})),
            ModelMessage::tool_result("call_1", serde_json::json!({"content": "x"}), false),
            ModelMessage::assistant("final"),
        ];

        let prepared = prepare_compaction(&messages, 8);

        assert!(prepared.split_turn);
        assert!(!prepared.turn_prefix_messages.is_empty());
        assert_eq!(
            prepared.turn_prefix_messages[0].role,
            Role::User,
            "split turn prefix should begin at the user message"
        );
        assert_eq!(prepared.messages_to_summarize.len(), 1);
        assert_eq!(prepared.messages_to_summarize[0].role, Role::System);
        assert_eq!(prepared.messages_to_summarize[0].text(), "You are helpful");
    }

    #[test]
    fn collect_entries_between_branches_reads_inclusive_range() {
        let messages = vec![
            ModelMessage::user("m0"),
            ModelMessage::assistant("m1"),
            ModelMessage::user("m2"),
            ModelMessage::assistant("m3"),
        ];

        let collected = collect_entries_between_branches(
            &messages,
            BranchEntryRange {
                start_entry_index: 1,
                end_entry_index: 2,
            },
        );

        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0].text(), "m1");
        assert_eq!(collected[1].text(), "m2");
    }
}
