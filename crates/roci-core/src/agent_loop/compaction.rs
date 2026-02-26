use std::collections::BTreeSet;

use crate::types::{ContentPart, ModelMessage, Role};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextUsage {
    pub used_tokens: usize,
    pub context_window: usize,
    pub remaining_tokens: usize,
    pub usage_percent: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileOperationSet {
    pub read_files: BTreeSet<String>,
    pub modified_files: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchEntryRange {
    pub start_entry_index: usize,
    pub end_entry_index: usize,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PreparedCompaction {
    pub messages_to_summarize: Vec<ModelMessage>,
    pub turn_prefix_messages: Vec<ModelMessage>,
    pub kept_messages: Vec<ModelMessage>,
    pub split_turn: bool,
    pub cut_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PiMonoSummary {
    pub goal: Vec<String>,
    pub constraints: Vec<String>,
    pub progress: Vec<String>,
    pub decisions: Vec<String>,
    pub next_steps: Vec<String>,
    pub critical_context: Vec<String>,
    pub read_files: BTreeSet<String>,
    pub modified_files: BTreeSet<String>,
}

const BRANCH_SUMMARY_PREFIX: &str = "<branch_summary>";
const BRANCH_SUMMARY_SUFFIX: &str = "</branch_summary>";

pub fn estimate_text_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.chars().count().div_ceil(4)
}

pub fn estimate_message_tokens(message: &ModelMessage) -> usize {
    let mut tokens = 4usize;
    for part in &message.content {
        tokens += match part {
            ContentPart::Text { text } => estimate_text_tokens(text),
            ContentPart::Image(image) => estimate_text_tokens(&image.data) + 8,
            ContentPart::ToolCall(tc) => {
                let args = serde_json::to_string(&tc.arguments).unwrap_or_default();
                estimate_text_tokens(&tc.name) + estimate_text_tokens(&args) + 8
            }
            ContentPart::ToolResult(result) => {
                let payload = serde_json::to_string(&result.result).unwrap_or_default();
                estimate_text_tokens(&result.tool_call_id) + estimate_text_tokens(&payload) + 8
            }
            ContentPart::Thinking(thinking) => {
                estimate_text_tokens(&thinking.thinking) + estimate_text_tokens(&thinking.signature)
            }
            ContentPart::RedactedThinking(thinking) => {
                estimate_text_tokens(&thinking.data) + estimate_text_tokens(&thinking.signature)
            }
        };
    }
    if let Some(name) = &message.name {
        tokens += estimate_text_tokens(name);
    }
    tokens
}

pub fn estimate_context_usage(messages: &[ModelMessage], context_window: usize) -> ContextUsage {
    let used_tokens = messages.iter().map(estimate_message_tokens).sum::<usize>();
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
        if used_tokens + message_tokens > token_budget {
            break;
        }
        used_tokens += message_tokens;
        selected.push(message.clone());
    }

    selected.reverse();
    selected
}

pub fn serialize_messages_for_summary(messages: &[ModelMessage]) -> String {
    let mut lines = Vec::new();

    for message in messages {
        for part in &message.content {
            match (message.role, part) {
                (Role::System, ContentPart::Text { text }) => {
                    lines.push(format!("[system] {text}"));
                }
                (Role::User, ContentPart::Text { text }) => {
                    lines.push(format!("[user] {text}"));
                }
                (Role::Assistant, ContentPart::Text { text }) => {
                    lines.push(format!("[assistant] {text}"));
                }
                (Role::Assistant, ContentPart::ToolCall(call)) => {
                    let args =
                        serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_string());
                    lines.push(format!("[assistant.tool_call] {} {}", call.name, args));
                }
                (Role::Tool, ContentPart::ToolResult(result)) => {
                    let payload = serde_json::to_string(&result.result)
                        .unwrap_or_else(|_| "null".to_string());
                    lines.push(format!(
                        "[tool] id={} is_error={} result={}",
                        result.tool_call_id, result.is_error, payload
                    ));
                }
                (_, ContentPart::Image(_)) => lines.push("[image] <omitted>".to_string()),
                (_, ContentPart::Thinking(_)) => lines.push("[thinking] <omitted>".to_string()),
                (_, ContentPart::RedactedThinking(_)) => {
                    lines.push("[redacted_thinking] <omitted>".to_string())
                }
                _ => {}
            }
        }
    }

    lines.join("\n")
}

pub fn serialize_pi_mono_summary(summary: &PiMonoSummary) -> String {
    fn section(title: &str, values: &[String]) -> String {
        if values.is_empty() {
            return format!("## {title}\n- (none)");
        }

        let bullets = values
            .iter()
            .map(|entry| format!("- {entry}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("## {title}\n{bullets}")
    }

    fn file_section(title: &str, values: &BTreeSet<String>) -> String {
        if values.is_empty() {
            return format!("### {title}\n- (none)");
        }

        let bullets = values
            .iter()
            .map(|entry| format!("- {entry}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("### {title}\n{bullets}")
    }

    [
        section("Goal", &summary.goal),
        section("Constraints", &summary.constraints),
        section("Progress", &summary.progress),
        section("Decisions", &summary.decisions),
        section("Next Steps", &summary.next_steps),
        section("Critical Context", &summary.critical_context),
        file_section("Read Files", &summary.read_files),
        file_section("Modified Files", &summary.modified_files),
    ]
    .join("\n\n")
}

pub fn extract_file_operations(messages: &[ModelMessage]) -> FileOperationSet {
    let mut file_ops = FileOperationSet::default();

    for message in messages {
        for part in &message.content {
            let ContentPart::ToolCall(call) = part else {
                continue;
            };

            let tool = call.name.as_str();

            if matches!(tool, "read_file" | "view" | "open_file" | "cat") {
                if let Some(path) = extract_path_argument(&call.arguments) {
                    file_ops.read_files.insert(path);
                }
                continue;
            }

            if matches!(
                tool,
                "write_file" | "edit_file" | "replace_in_file" | "create_file" | "delete_file"
            ) {
                if let Some(path) = extract_path_argument(&call.arguments) {
                    file_ops.modified_files.insert(path);
                }
                continue;
            }

            if tool == "apply_patch" {
                if let Some(patch) = call
                    .arguments
                    .get("patch")
                    .and_then(serde_json::Value::as_str)
                {
                    for line in patch.lines() {
                        if let Some(path) = line.strip_prefix("*** Update File: ") {
                            file_ops.modified_files.insert(path.trim().to_string());
                        } else if let Some(path) = line.strip_prefix("*** Add File: ") {
                            file_ops.modified_files.insert(path.trim().to_string());
                        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
                            file_ops.modified_files.insert(path.trim().to_string());
                        } else if let Some(path) = line.strip_prefix("*** Move to: ") {
                            file_ops.modified_files.insert(path.trim().to_string());
                        }
                    }
                }
            }
        }
    }

    file_ops
}

pub fn extract_cumulative_file_operations(
    prior_summary_messages: &[ModelMessage],
    messages_to_summarize: &[ModelMessage],
) -> FileOperationSet {
    let mut file_ops = extract_file_operations(messages_to_summarize);
    let historical_ops = extract_file_ops_from_branch_summaries(prior_summary_messages);
    file_ops.read_files.extend(historical_ops.read_files);
    file_ops
        .modified_files
        .extend(historical_ops.modified_files);
    file_ops
}

fn extract_file_ops_from_branch_summaries(messages: &[ModelMessage]) -> FileOperationSet {
    let mut file_ops = FileOperationSet::default();

    for message in messages {
        if message.role != Role::User {
            continue;
        }
        let text = message.text();
        let Some(summary) = unwrap_branch_summary(&text) else {
            continue;
        };
        let summary_ops = parse_pi_mono_summary_file_sections(summary);
        file_ops.read_files.extend(summary_ops.read_files);
        file_ops.modified_files.extend(summary_ops.modified_files);
    }

    file_ops
}

fn unwrap_branch_summary(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    let inner = trimmed
        .strip_prefix(BRANCH_SUMMARY_PREFIX)?
        .strip_suffix(BRANCH_SUMMARY_SUFFIX)?;
    Some(inner.trim())
}

fn parse_pi_mono_summary_file_sections(summary: &str) -> FileOperationSet {
    let mut file_ops = FileOperationSet::default();
    let mut active_section: Option<&str> = None;

    for line in summary.lines() {
        let trimmed = line.trim();
        if trimmed == "### Read Files" {
            active_section = Some("read");
            continue;
        }
        if trimmed == "### Modified Files" {
            active_section = Some("modified");
            continue;
        }
        if trimmed.starts_with("## ") || trimmed.starts_with("### ") {
            active_section = None;
            continue;
        }
        let Some(value) = trimmed.strip_prefix("- ").map(str::trim) else {
            continue;
        };
        if value.is_empty() || value == "(none)" {
            continue;
        }

        match active_section {
            Some("read") => {
                file_ops.read_files.insert(value.to_string());
            }
            Some("modified") => {
                file_ops.modified_files.insert(value.to_string());
            }
            _ => {}
        }
    }

    file_ops
}

fn extract_path_argument(arguments: &serde_json::Value) -> Option<String> {
    for key in [
        "path",
        "file_path",
        "filepath",
        "file",
        "target_file",
        "from",
        "to",
    ] {
        if let Some(path) = arguments.get(key).and_then(serde_json::Value::as_str) {
            if !path.trim().is_empty() {
                return Some(path.to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentToolCall;

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

    #[test]
    fn select_messages_with_token_budget_prefers_newest_messages() {
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
    fn serialize_messages_for_summary_includes_roles_and_tool_data() {
        let messages = vec![
            ModelMessage::user("hello"),
            assistant_with_tool_call("write_file", serde_json::json!({"path": "src/lib.rs"})),
            ModelMessage::tool_result("call_1", serde_json::json!({"ok": true}), false),
        ];

        let serialized = serialize_messages_for_summary(&messages);

        assert!(serialized.contains("[user] hello"));
        assert!(serialized.contains("[assistant.tool_call] write_file"));
        assert!(serialized.contains("[tool]"));
        assert!(serialized.contains("\"ok\":true"));
    }

    #[test]
    fn serialize_pi_mono_summary_uses_expected_sections() {
        let summary = PiMonoSummary {
            goal: vec!["Ship compaction".to_string()],
            constraints: vec!["No settings loading in this task".to_string()],
            progress: vec!["Added utility layer".to_string()],
            decisions: vec!["Use summary message variants".to_string()],
            next_steps: vec!["Wire execution hooks".to_string()],
            critical_context: vec!["Do not cut at tool results".to_string()],
            read_files: BTreeSet::from(["src/a.rs".to_string()]),
            modified_files: BTreeSet::from(["src/b.rs".to_string()]),
        };

        let serialized = serialize_pi_mono_summary(&summary);

        assert!(serialized.contains("## Goal"));
        assert!(serialized.contains("## Constraints"));
        assert!(serialized.contains("## Progress"));
        assert!(serialized.contains("## Decisions"));
        assert!(serialized.contains("## Next Steps"));
        assert!(serialized.contains("## Critical Context"));
        assert!(serialized.contains("### Read Files"));
        assert!(serialized.contains("- src/a.rs"));
        assert!(serialized.contains("### Modified Files"));
        assert!(serialized.contains("- src/b.rs"));
    }

    #[test]
    fn extract_file_operations_captures_read_and_modified_paths() {
        let messages = vec![
            assistant_with_tool_call("read_file", serde_json::json!({"path": "src/main.rs"})),
            assistant_with_tool_call("write_file", serde_json::json!({"path": "src/lib.rs"})),
            assistant_with_tool_call(
                "apply_patch",
                serde_json::json!({"patch": "*** Begin Patch\n*** Update File: src/core.rs\n*** End Patch\n"}),
            ),
        ];

        let file_ops = extract_file_operations(&messages);

        assert!(file_ops.read_files.contains("src/main.rs"));
        assert!(file_ops.modified_files.contains("src/lib.rs"));
        assert!(file_ops.modified_files.contains("src/core.rs"));
    }

    #[test]
    fn extract_cumulative_file_operations_merges_prior_branch_summary_files() {
        let prior_summary = PiMonoSummary {
            read_files: BTreeSet::from(["src/old_read.rs".to_string()]),
            modified_files: BTreeSet::from(["src/old_mod.rs".to_string()]),
            ..PiMonoSummary::default()
        };
        let prior_message = ModelMessage::user(format!(
            "{BRANCH_SUMMARY_PREFIX}\n{}\n{BRANCH_SUMMARY_SUFFIX}",
            serialize_pi_mono_summary(&prior_summary)
        ));
        let new_messages = vec![
            assistant_with_tool_call("read_file", serde_json::json!({"path": "src/new_read.rs"})),
            assistant_with_tool_call("write_file", serde_json::json!({"path": "src/new_mod.rs"})),
        ];

        let file_ops = extract_cumulative_file_operations(&[prior_message], &new_messages);

        assert!(file_ops.read_files.contains("src/old_read.rs"));
        assert!(file_ops.read_files.contains("src/new_read.rs"));
        assert!(file_ops.modified_files.contains("src/old_mod.rs"));
        assert!(file_ops.modified_files.contains("src/new_mod.rs"));
    }
}
