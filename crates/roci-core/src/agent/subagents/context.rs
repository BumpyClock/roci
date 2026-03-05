//! Context materialization and child message composition for sub-agents.

use super::prompt::SubagentPromptPolicy;
use super::types::{
    SnapshotMode, SubagentContext, SubagentInput, SubagentOverrides, SubagentProfile,
};
use crate::types::{ModelMessage, Role};

/// Materialize parent context into a [`SubagentContext`] based on the snapshot mode.
///
/// - `SummaryOnly` uses the provided `summary` parameter.
/// - `SelectedMessages` uses the caller-provided explicit messages.
/// - `FullReadonlySnapshot` clones all parent messages, excluding runtime internals
///   (tool-role messages).
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
            let filtered: Vec<ModelMessage> = parent_messages
                .iter()
                .filter(|m| m.role != Role::Tool)
                .cloned()
                .collect();
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
/// 2. Context messages (summary as system msg, selected_messages appended) if snapshot is present.
/// 3. User message with `task` if a prompt is present.
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
        messages.push(ModelMessage::system(format!(
            "Parent context summary:\n{summary}"
        )));
    }
    messages.extend(context.selected_messages.iter().cloned());
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // materialize_context
    // -----------------------------------------------------------------------

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
    fn materialize_selected_messages_returns_explicit() {
        let explicit = vec![ModelMessage::user("hello"), ModelMessage::assistant("hi")];
        let mode = SnapshotMode::SelectedMessages(explicit.clone());
        let ctx = materialize_context(&[], &mode, None);
        assert!(ctx.summary.is_none());
        assert_eq!(ctx.selected_messages.len(), 2);
        assert_eq!(ctx.selected_messages[0].text(), "hello");
        assert_eq!(ctx.selected_messages[1].text(), "hi");
    }

    #[test]
    fn materialize_full_snapshot_clones_parent_messages() {
        let parent = vec![
            ModelMessage::system("sys"),
            ModelMessage::user("u1"),
            ModelMessage::assistant("a1"),
        ];
        let ctx = materialize_context(&parent, &SnapshotMode::FullReadonlySnapshot, None);
        assert!(ctx.summary.is_none());
        assert_eq!(ctx.selected_messages.len(), 3);
    }

    #[test]
    fn materialize_full_snapshot_excludes_tool_messages() {
        let parent = vec![
            ModelMessage::user("u1"),
            ModelMessage::tool_result("tc1", serde_json::json!({"ok": true}), false),
            ModelMessage::assistant("a1"),
        ];
        let ctx = materialize_context(&parent, &SnapshotMode::FullReadonlySnapshot, None);
        // Tool message should be filtered out
        assert_eq!(ctx.selected_messages.len(), 2);
        assert_eq!(ctx.selected_messages[0].text(), "u1");
        assert_eq!(ctx.selected_messages[1].text(), "a1");
    }

    // -----------------------------------------------------------------------
    // build_child_initial_messages
    // -----------------------------------------------------------------------

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
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::System);
        assert_eq!(msgs[1].role, Role::System);
        assert!(msgs[1].text().contains("parent summary"));
    }

    #[test]
    fn build_messages_snapshot_mode_with_selected_messages() {
        let (policy, profile, overrides) = setup();
        let input = SubagentInput::Snapshot {
            mode: SnapshotMode::SummaryOnly,
        };
        let ctx = SubagentContext {
            selected_messages: vec![ModelMessage::user("ctx msg")],
            ..Default::default()
        };
        let msgs = build_child_initial_messages(&input, &ctx, &policy, &profile, &overrides);
        // system + selected message
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].role, Role::User);
        assert_eq!(msgs[1].text(), "ctx msg");
    }

    #[test]
    fn build_messages_prompt_with_snapshot_mode() {
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
        // system + summary system + selected msg + user task
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, Role::System);
        assert!(msgs[1].text().contains("conversation so far"));
        assert_eq!(msgs[2].text(), "earlier msg");
        assert_eq!(msgs[3].role, Role::User);
        assert_eq!(msgs[3].text(), "implement feature");
    }

    // -----------------------------------------------------------------------
    // default_child_input
    // -----------------------------------------------------------------------

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
