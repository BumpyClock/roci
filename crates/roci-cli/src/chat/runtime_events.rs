use std::collections::HashMap;
use std::io::Write;
use std::time::Duration;

use roci::agent::{
    AgentRuntimeEventPayload, MessageId, MessageSnapshot, RuntimeSubscription,
    ToolExecutionSnapshot,
};
use roci::types::{ContentPart, Role};

use super::resource_prompt::truncate_preview;

pub(crate) struct RuntimeEventRenderer {
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    handle: tokio::task::JoinHandle<()>,
}

impl RuntimeEventRenderer {
    pub(crate) fn spawn(subscription: RuntimeSubscription) -> Self {
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move {
            drive_chat_renderer(subscription, shutdown_rx).await;
        });

        Self {
            shutdown_tx: Some(shutdown_tx),
            handle,
        }
    }

    pub(crate) async fn finish(mut self) {
        if tokio::time::timeout(Duration::from_secs(1), &mut self.handle)
            .await
            .is_err()
        {
            if let Some(shutdown_tx) = self.shutdown_tx.take() {
                let _ = shutdown_tx.send(());
            }
            let _ = self.handle.await;
        }
    }
}

async fn drive_chat_renderer(
    mut subscription: RuntimeSubscription,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let mut renderer = ChatRenderer::default();

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => break,
            event = subscription.recv() => match event {
                Ok(event) => {
                    if renderer.render_payload(event.payload) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }
}

#[derive(Default)]
struct ChatRenderer {
    printed_text_by_message_id: HashMap<MessageId, String>,
}

impl ChatRenderer {
    fn render_payload(&mut self, payload: AgentRuntimeEventPayload) -> bool {
        let mut stdout = std::io::stdout();
        let mut stderr = std::io::stderr();
        self.render_payload_to(payload, &mut stdout, &mut stderr)
    }

    fn render_payload_to(
        &mut self,
        payload: AgentRuntimeEventPayload,
        stdout: &mut impl Write,
        stderr: &mut impl Write,
    ) -> bool {
        match payload {
            AgentRuntimeEventPayload::MessageStarted { message }
            | AgentRuntimeEventPayload::MessageUpdated { message }
            | AgentRuntimeEventPayload::MessageCompleted { message } => {
                self.render_message_snapshot(message, stdout);
            }
            AgentRuntimeEventPayload::ToolStarted { tool } => {
                let _ = writeln!(stderr, "\n⚡ {} ({})", tool.tool_name, tool.tool_call_id);
            }
            AgentRuntimeEventPayload::ToolUpdated { tool } => {
                self.render_tool_update(tool, stderr);
            }
            AgentRuntimeEventPayload::ToolCompleted { tool } => {
                self.render_tool_completion(tool, stderr);
            }
            AgentRuntimeEventPayload::TurnCompleted { .. }
            | AgentRuntimeEventPayload::TurnFailed { .. }
            | AgentRuntimeEventPayload::TurnCanceled { .. } => return true,
            AgentRuntimeEventPayload::TurnQueued { .. }
            | AgentRuntimeEventPayload::TurnStarted { .. } => {}
        }

        false
    }

    fn render_message_snapshot(&mut self, message: MessageSnapshot, stdout: &mut impl Write) {
        if message.payload.role != Role::Assistant {
            self.printed_text_by_message_id.remove(&message.message_id);
            return;
        }

        let text = message.payload.text();
        let printed = self
            .printed_text_by_message_id
            .entry(message.message_id)
            .or_default();

        if text.starts_with(printed.as_str()) {
            let suffix = &text[printed.len()..];
            if !suffix.is_empty() {
                let _ = write!(stdout, "{suffix}");
                let _ = stdout.flush();
                printed.push_str(suffix);
            }
        } else if printed.is_empty() && !text.is_empty() {
            let _ = write!(stdout, "{text}");
            let _ = stdout.flush();
            printed.clone_from(&text);
        } else {
            printed.clone_from(&text);
        }

        if matches!(message.status, roci::agent::MessageStatus::Completed) {
            self.printed_text_by_message_id.remove(&message.message_id);
        }
    }

    fn render_tool_update(&self, tool: ToolExecutionSnapshot, stderr: &mut impl Write) {
        let Some(partial_result) = tool.partial_result else {
            return;
        };
        let preview = if let Some(text) = partial_result.content.iter().find_map(|part| {
            if let ContentPart::Text { text } = part {
                Some(text.as_str())
            } else {
                None
            }
        }) {
            truncate_preview(text, 80)
        } else {
            truncate_preview(&partial_result.details.to_string(), 80)
        };
        let _ = writeln!(stderr, "  … {}: {preview}", tool.tool_name);
    }

    fn render_tool_completion(&self, tool: ToolExecutionSnapshot, stderr: &mut impl Write) {
        let Some(result) = tool.final_result else {
            return;
        };
        let preview = truncate_preview(&result.result.to_string(), 200);
        if result.is_error {
            let _ = writeln!(stderr, "  ❌ {preview}");
        } else {
            let _ = writeln!(stderr, "  ✅ {preview}");
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use roci::agent::{MessageStatus, ThreadId, ToolStatus, TurnId, TurnSnapshot, TurnStatus};
    use roci::agent_loop::ToolUpdatePayload;
    use roci::types::{AgentToolResult, ModelMessage};

    fn assistant_message(
        thread_id: ThreadId,
        ordinal: u64,
        status: MessageStatus,
        text: &str,
    ) -> MessageSnapshot {
        let turn_id = TurnId::new(thread_id, 1, 1);
        MessageSnapshot {
            message_id: MessageId::new(thread_id, 1, ordinal),
            thread_id,
            turn_id,
            status,
            payload: ModelMessage::assistant(text),
            created_at: Utc::now(),
            completed_at: (status == MessageStatus::Completed).then(Utc::now),
        }
    }

    fn user_message(thread_id: ThreadId, ordinal: u64, text: &str) -> MessageSnapshot {
        let turn_id = TurnId::new(thread_id, 1, 1);
        MessageSnapshot {
            message_id: MessageId::new(thread_id, 1, ordinal),
            thread_id,
            turn_id,
            status: MessageStatus::Completed,
            payload: ModelMessage::user(text),
            created_at: Utc::now(),
            completed_at: Some(Utc::now()),
        }
    }

    fn tool_snapshot(thread_id: ThreadId) -> ToolExecutionSnapshot {
        ToolExecutionSnapshot {
            tool_call_id: "call_1".to_string(),
            thread_id,
            turn_id: TurnId::new(thread_id, 1, 1),
            tool_name: "search".to_string(),
            args: serde_json::json!({ "query": "roci" }),
            status: ToolStatus::Running,
            partial_result: None,
            final_result: None,
            started_at: Utc::now(),
            completed_at: None,
        }
    }

    fn completed_turn(thread_id: ThreadId) -> TurnSnapshot {
        TurnSnapshot {
            turn_id: TurnId::new(thread_id, 1, 1),
            thread_id,
            status: TurnStatus::Completed,
            message_ids: Vec::new(),
            active_tool_call_ids: Vec::new(),
            error: None,
            queued_at: Utc::now(),
            started_at: Some(Utc::now()),
            completed_at: Some(Utc::now()),
        }
    }

    #[test]
    fn chat_renderer_prints_only_incremental_assistant_text() {
        let thread_id = ThreadId::new();
        let mut renderer = ChatRenderer::default();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let should_stop = renderer.render_payload_to(
            AgentRuntimeEventPayload::MessageStarted {
                message: user_message(thread_id, 1, "ignore"),
            },
            &mut stdout,
            &mut stderr,
        );
        assert!(!should_stop);

        renderer.render_payload_to(
            AgentRuntimeEventPayload::MessageStarted {
                message: assistant_message(thread_id, 2, MessageStatus::Streaming, "Hel"),
            },
            &mut stdout,
            &mut stderr,
        );
        renderer.render_payload_to(
            AgentRuntimeEventPayload::MessageUpdated {
                message: assistant_message(thread_id, 2, MessageStatus::Streaming, "Hello"),
            },
            &mut stdout,
            &mut stderr,
        );
        renderer.render_payload_to(
            AgentRuntimeEventPayload::MessageCompleted {
                message: assistant_message(thread_id, 2, MessageStatus::Completed, "Hello!"),
            },
            &mut stdout,
            &mut stderr,
        );

        assert_eq!(String::from_utf8(stdout).unwrap(), "Hello!");
        assert!(String::from_utf8(stderr).unwrap().is_empty());
        assert!(renderer.printed_text_by_message_id.is_empty());
    }

    #[test]
    fn chat_renderer_renders_tool_events_and_stops_on_terminal_turn() {
        let thread_id = ThreadId::new();
        let mut renderer = ChatRenderer::default();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let mut running_tool = tool_snapshot(thread_id);
        let mut completed_tool = running_tool.clone();
        running_tool.partial_result = Some(ToolUpdatePayload {
            content: vec![ContentPart::Text {
                text: "step 1".to_string(),
            }],
            details: serde_json::Value::Null,
        });
        completed_tool.status = ToolStatus::Completed;
        completed_tool.partial_result = running_tool.partial_result.clone();
        completed_tool.final_result = Some(AgentToolResult {
            tool_call_id: "call_1".to_string(),
            result: serde_json::json!({ "ok": true }),
            is_error: false,
        });
        completed_tool.completed_at = Some(Utc::now());

        assert!(!renderer.render_payload_to(
            AgentRuntimeEventPayload::ToolStarted {
                tool: tool_snapshot(thread_id),
            },
            &mut stdout,
            &mut stderr,
        ));
        assert!(!renderer.render_payload_to(
            AgentRuntimeEventPayload::ToolUpdated { tool: running_tool },
            &mut stdout,
            &mut stderr,
        ));
        assert!(!renderer.render_payload_to(
            AgentRuntimeEventPayload::ToolCompleted {
                tool: completed_tool,
            },
            &mut stdout,
            &mut stderr,
        ));
        assert!(renderer.render_payload_to(
            AgentRuntimeEventPayload::TurnCompleted {
                turn: completed_turn(thread_id),
            },
            &mut stdout,
            &mut stderr,
        ));

        assert!(String::from_utf8(stdout).unwrap().is_empty());
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            "\n⚡ search (call_1)\n  … search: step 1\n  ✅ {\"ok\":true}\n"
        );
    }
}
