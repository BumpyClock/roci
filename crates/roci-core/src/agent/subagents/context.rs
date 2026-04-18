//! Context materialization and child message composition for sub-agents.

use super::prompt::SubagentPromptPolicy;
use super::types::{
    SnapshotMode, SubagentContext, SubagentInput, SubagentOverrides, SubagentProfile,
};
use crate::types::{ModelMessage, Role};

const SNAPSHOT_CONTINUATION_PROMPT: &str =
    "You are continuing from a read-only snapshot of parent context. No new task was supplied. \
     Treat the snapshot as background context, not as live authoritative state. \
     If it contains a clear unfinished task, continue that work and report progress to the parent. \
     Otherwise, return a concise status summary and the single best next action. \
     Do not address the end user directly. Re-read files or re-check live state before making changes.";
const FULL_READONLY_SNAPSHOT_RECENT_MESSAGE_LIMIT: usize = 8;

/// Materialize parent context into a [`SubagentContext`] based on the snapshot mode.
///
/// - `SummaryOnly` uses the provided `summary` parameter.
/// - `SelectedMessages` uses the caller-provided explicit messages.
/// - `FullReadonlySnapshot` clones up to
///   `FULL_READONLY_SNAPSHOT_RECENT_MESSAGE_LIMIT` of the most recent
///   user/assistant parent messages, excluding runtime internals and parent
///   instructions (tool-role and system-role messages).
pub fn materialize_context(
    parent_messages: &[ModelMessage],
    mode: &SnapshotMode,
    summary: Option<String>,
) -> SubagentContext {
    match mode {
        SnapshotMode::SummaryOnly => SubagentContext {
            summary,
            ..Default::default()
        },
        SnapshotMode::SelectedMessages(messages) => SubagentContext {
            selected_messages: messages.clone(),
            ..Default::default()
        },
        SnapshotMode::FullReadonlySnapshot => {
            let mut filtered: Vec<ModelMessage> = parent_messages
                .iter()
                .filter(|message| matches!(message.role, Role::User | Role::Assistant))
                .cloned()
                .collect();

            if filtered.len() > FULL_READONLY_SNAPSHOT_RECENT_MESSAGE_LIMIT {
                let retained_start = filtered.len() - FULL_READONLY_SNAPSHOT_RECENT_MESSAGE_LIMIT;
                filtered = filtered.split_off(retained_start);
            }

            SubagentContext {
                selected_messages: filtered,
                ..Default::default()
            }
        }
    }
}

/// Build the initial messages for a child sub-agent based on input mode.
///
/// Message ordering:
/// 1. System message from `prompt_policy.build_system_prompt(profile, overrides)`.
/// 2. Read-only context messages, if a snapshot is present.
/// 3. Executable user prompt:
///    - explicit `task` for prompt-bearing modes
///    - fixed continuation prompt for snapshot-only mode
pub fn build_child_initial_messages(
    input: &SubagentInput,
    context: &SubagentContext,
    prompt_policy: &SubagentPromptPolicy,
    profile: &SubagentProfile,
    overrides: &SubagentOverrides,
) -> Vec<ModelMessage> {
    let system_prompt = prompt_policy.build_system_prompt(profile, overrides);
    let mut messages = vec![ModelMessage::system(system_prompt)];

    match input {
        SubagentInput::Prompt { task } => {
            messages.push(ModelMessage::user(task));
        }
        SubagentInput::Snapshot { .. } => {
            append_context_messages(&mut messages, context);
            messages.push(ModelMessage::user(SNAPSHOT_CONTINUATION_PROMPT));
        }
        SubagentInput::PromptWithSnapshot { task, .. } => {
            append_context_messages(&mut messages, context);
            messages.push(ModelMessage::user(task));
        }
    }

    messages
}

/// Default child input: prompt + summary-only snapshot.
pub fn default_child_input(task: String) -> SubagentInput {
    SubagentInput::PromptWithSnapshot {
        task,
        mode: SnapshotMode::SummaryOnly,
    }
}

/// Append context (summary and/or selected messages) to the message list.
fn append_context_messages(messages: &mut Vec<ModelMessage>, context: &SubagentContext) {
    if let Some(summary) = &context.summary {
        messages.push(ModelMessage::user(format!(
            "Parent context summary:\n{summary}"
        )));
    }
    messages.extend(context.selected_messages.iter().cloned());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materialize_summary_only_returns_summary() {
        let ctx = materialize_context(&[], &SnapshotMode::SummaryOnly, Some("sum".into()));
        assert_eq!(ctx.summary.as_deref(), Some("sum"));
        assert!(ctx.selected_messages.is_empty());
    }

    #[test]
    fn materialize_summary_only_without_summary() {
        let ctx = materialize_context(&[], &SnapshotMode::SummaryOnly, None);
        assert!(ctx.summary.is_none());
        assert!(ctx.selected_messages.is_empty());
    }

    #[test]
    fn materialize_selected_messages_returns_exact_pass_through() {
        let explicit = vec![
            ModelMessage::system("preserved system"),
            ModelMessage::user("hello"),
            ModelMessage::tool_result("tool-1", serde_json::json!({ "ok": true }), false),
            ModelMessage::assistant("hi"),
        ];
        let mode = SnapshotMode::SelectedMessages(explicit.clone());
        let ctx = materialize_context(&[], &mode, None);
        assert!(ctx.summary.is_none());
        assert_eq!(ctx.selected_messages, explicit);
    }

    #[test]
    fn materialize_full_snapshot_keeps_only_user_and_assistant_messages() {
        let parent = vec![
            ModelMessage::system("sys"),
            ModelMessage::user("u1"),
            ModelMessage::tool_result("tool-1", serde_json::json!({ "ok": true }), false),
            ModelMessage::assistant("a1"),
            ModelMessage::user("u2"),
        ];
        let ctx = materialize_context(&parent, &SnapshotMode::FullReadonlySnapshot, None);
        assert!(ctx.summary.is_none());
        assert_eq!(ctx.selected_messages.len(), 3);
        assert_eq!(ctx.selected_messages[0].text(), "u1");
        assert_eq!(ctx.selected_messages[1].text(), "a1");
        assert_eq!(ctx.selected_messages[2].text(), "u2");
        assert!(ctx
            .selected_messages
            .iter()
            .all(|message| matches!(message.role, Role::User | Role::Assistant)));
    }

    #[test]
    fn materialize_full_snapshot_keeps_only_newest_user_and_assistant_messages() {
        let parent = vec![
            ModelMessage::system("sys-0"),
            ModelMessage::user("u1"),
            ModelMessage::assistant("a2"),
            ModelMessage::tool_result("tool-1", serde_json::json!({ "ignored": true }), false),
            ModelMessage::user("u3"),
            ModelMessage::assistant("a4"),
            ModelMessage::system("sys-1"),
            ModelMessage::user("u5"),
            ModelMessage::assistant("a6"),
            ModelMessage::tool_result("tool-2", serde_json::json!({ "ignored": true }), false),
            ModelMessage::user("u7"),
            ModelMessage::assistant("a8"),
            ModelMessage::user("u9"),
            ModelMessage::assistant("a10"),
        ];
        let ctx = materialize_context(&parent, &SnapshotMode::FullReadonlySnapshot, None);
        let retained_text: Vec<String> = ctx
            .selected_messages
            .iter()
            .map(ModelMessage::text)
            .collect();

        assert_eq!(
            ctx.selected_messages.len(),
            FULL_READONLY_SNAPSHOT_RECENT_MESSAGE_LIMIT
        );
        assert_eq!(
            retained_text,
            vec!["u3", "a4", "u5", "a6", "u7", "a8", "u9", "a10"]
        );
        assert!(ctx
            .selected_messages
            .iter()
            .all(|message| matches!(message.role, Role::User | Role::Assistant)));
    }

    fn setup() -> (SubagentPromptPolicy, SubagentProfile, SubagentOverrides) {
        (
            SubagentPromptPolicy::default(),
            SubagentProfile::builtin_developer(),
            SubagentOverrides::default(),
        )
    }

    #[test]
    fn build_messages_prompt_mode_system_plus_user() {
        let (policy, profile, overrides) = setup();
        let input = SubagentInput::Prompt {
            task: "fix the bug".into(),
        };
        let ctx = SubagentContext::default();
        let msgs = build_child_initial_messages(&input, &ctx, &policy, &profile, &overrides);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::System);
        assert_eq!(msgs[1].role, Role::User);
        assert_eq!(msgs[1].text(), "fix the bug");
    }

    #[test]
    fn build_messages_snapshot_mode_system_plus_context() {
        let (policy, profile, overrides) = setup();
        let input = SubagentInput::Snapshot {
            mode: SnapshotMode::SummaryOnly,
        };
        let ctx = SubagentContext {
            summary: Some("parent summary".into()),
            ..Default::default()
        };
        let msgs = build_child_initial_messages(&input, &ctx, &policy, &profile, &overrides);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, Role::System);
        assert_eq!(msgs[1].role, Role::User);
        assert!(msgs[1].text().contains("parent summary"));
        assert_eq!(
            msgs[2].text(),
            "You are continuing from a read-only snapshot of parent context. No new task was supplied. Treat the snapshot as background context, not as live authoritative state. If it contains a clear unfinished task, continue that work and report progress to the parent. Otherwise, return a concise status summary and the single best next action. Do not address the end user directly. Re-read files or re-check live state before making changes."
        );
    }

    #[test]
    fn build_messages_snapshot_mode_preserves_selected_messages_before_synthetic_prompt() {
        let (policy, profile, overrides) = setup();
        let input = SubagentInput::Snapshot {
            mode: SnapshotMode::SelectedMessages(vec![
                ModelMessage::assistant("previous answer"),
                ModelMessage::user("follow-up"),
            ]),
        };
        let ctx = SubagentContext {
            selected_messages: vec![
                ModelMessage::assistant("previous answer"),
                ModelMessage::user("follow-up"),
            ],
            ..Default::default()
        };
        let msgs = build_child_initial_messages(&input, &ctx, &policy, &profile, &overrides);
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[1].role, Role::Assistant);
        assert_eq!(msgs[1].text(), "previous answer");
        assert_eq!(msgs[2].role, Role::User);
        assert_eq!(msgs[2].text(), "follow-up");
        assert_eq!(
            msgs[3].text(),
            "You are continuing from a read-only snapshot of parent context. No new task was supplied. Treat the snapshot as background context, not as live authoritative state. If it contains a clear unfinished task, continue that work and report progress to the parent. Otherwise, return a concise status summary and the single best next action. Do not address the end user directly. Re-read files or re-check live state before making changes."
        );
    }

    #[test]
    fn build_messages_prompt_with_snapshot_places_explicit_task_after_context() {
        let (policy, profile, overrides) = setup();
        let input = SubagentInput::PromptWithSnapshot {
            task: "implement feature".into(),
            mode: SnapshotMode::SummaryOnly,
        };
        let ctx = SubagentContext {
            summary: Some("conversation so far".into()),
            selected_messages: vec![ModelMessage::user("earlier msg")],
            ..Default::default()
        };
        let msgs = build_child_initial_messages(&input, &ctx, &policy, &profile, &overrides);
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, Role::System);
        assert!(msgs[1].text().contains("conversation so far"));
        assert_eq!(msgs[2].text(), "earlier msg");
        assert_eq!(msgs[3].role, Role::User);
        assert_eq!(msgs[3].text(), "implement feature");
    }

    #[test]
    fn default_child_input_produces_prompt_with_snapshot_summary_only() {
        let input = default_child_input("do something".into());
        match input {
            SubagentInput::PromptWithSnapshot { task, mode } => {
                assert_eq!(task, "do something");
                assert!(matches!(mode, SnapshotMode::SummaryOnly));
            }
            _ => panic!("expected PromptWithSnapshot variant"),
        }
    }
}
