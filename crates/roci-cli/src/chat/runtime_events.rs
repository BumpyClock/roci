use std::collections::{HashMap, HashSet};
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;

use roci::agent::{
    AgentRuntime, AgentRuntimeError, AgentRuntimeEventPayload, MessageId, MessageSnapshot,
    RuntimeCursor, RuntimeSnapshot, RuntimeSubscription, ToolExecutionSnapshot,
    UserInputCoordinator,
};
use roci::agent_loop::{AgentEvent, ApprovalDecision, ApprovalHandler, ApprovalRequest};
use roci::tools::UserInputRequest;
use roci::types::{ContentPart, Role};
use tokio::task::JoinHandle as TaskJoinHandle;

use super::resource_prompt::truncate_preview;
use super::user_input::{default_prompt_fn, handle_prompt_request, PromptFn};

type ApprovalPromptFn = Arc<dyn Fn(ApprovalRequest) -> ApprovalDecision + Send + Sync>;

enum TerminalCommand {
    RuntimeEvent(Box<AgentRuntimeEventPayload>),
    UserInputRequest(UserInputRequest),
    ApprovalRequest {
        request: ApprovalRequest,
        response_tx: tokio::sync::oneshot::Sender<ApprovalDecision>,
    },
    Snapshot(RuntimeSnapshot),
    StreamError(AgentRuntimeError),
    Shutdown,
}

pub(crate) struct RuntimeEventRenderer {
    command_tx: mpsc::Sender<TerminalCommand>,
    shutdown: Arc<AtomicBool>,
    subscription_handle: Option<TaskJoinHandle<()>>,
    terminal_handle: Option<JoinHandle<()>>,
}

impl RuntimeEventRenderer {
    pub(crate) fn spawn(coordinator: Arc<UserInputCoordinator>) -> Self {
        Self::spawn_with_prompt_fns(
            coordinator,
            default_prompt_fn(),
            default_approval_prompt_fn(),
        )
    }

    #[cfg(test)]
    pub(crate) fn spawn_with_prompt_fn(
        coordinator: Arc<UserInputCoordinator>,
        prompt_fn: PromptFn,
    ) -> Self {
        Self::spawn_with_prompt_fns(coordinator, prompt_fn, default_approval_prompt_fn())
    }

    #[cfg(test)]
    pub(crate) fn spawn_with_prompt_fns(
        coordinator: Arc<UserInputCoordinator>,
        prompt_fn: PromptFn,
        approval_prompt_fn: ApprovalPromptFn,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = shutdown.clone();
        let handle = tokio::runtime::Handle::current();
        let terminal_handle = std::thread::spawn(move || {
            drive_terminal(
                command_rx,
                coordinator,
                prompt_fn,
                approval_prompt_fn,
                handle,
                thread_shutdown,
            );
        });

        Self {
            command_tx,
            shutdown,
            subscription_handle: None,
            terminal_handle: Some(terminal_handle),
        }
    }

    #[cfg(not(test))]
    fn spawn_with_prompt_fns(
        coordinator: Arc<UserInputCoordinator>,
        prompt_fn: PromptFn,
        approval_prompt_fn: ApprovalPromptFn,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = shutdown.clone();
        let handle = tokio::runtime::Handle::current();
        let terminal_handle = std::thread::spawn(move || {
            drive_terminal(
                command_rx,
                coordinator,
                prompt_fn,
                approval_prompt_fn,
                handle,
                thread_shutdown,
            );
        });

        Self {
            command_tx,
            shutdown,
            subscription_handle: None,
            terminal_handle: Some(terminal_handle),
        }
    }

    pub(crate) fn build_agent_sink(&self) -> Arc<dyn Fn(AgentEvent) + Send + Sync> {
        let command_tx = self.command_tx.clone();
        Arc::new(move |event: AgentEvent| {
            if let AgentEvent::UserInputRequested { request } = event {
                let _ = command_tx.send(TerminalCommand::UserInputRequest(request));
            }
        })
    }

    pub(crate) fn build_approval_handler(&self) -> ApprovalHandler {
        let command_tx = self.command_tx.clone();
        let shutdown = self.shutdown.clone();
        Arc::new(move |request: ApprovalRequest| {
            let command_tx = command_tx.clone();
            let shutdown = shutdown.clone();
            Box::pin(async move {
                if shutdown.load(Ordering::Relaxed) {
                    return ApprovalDecision::Cancel;
                }
                let (response_tx, response_rx) = tokio::sync::oneshot::channel();
                if command_tx
                    .send(TerminalCommand::ApprovalRequest {
                        request,
                        response_tx,
                    })
                    .is_err()
                {
                    return ApprovalDecision::Decline;
                }
                response_rx.await.unwrap_or(ApprovalDecision::Decline)
            })
        })
    }

    pub(crate) fn subscribe(
        &mut self,
        subscription: RuntimeSubscription,
        agent: Arc<AgentRuntime>,
    ) {
        let command_tx = self.command_tx.clone();
        self.subscription_handle = Some(tokio::spawn(async move {
            drive_runtime_subscription(subscription, agent, command_tx).await;
        }));
    }

    pub(crate) async fn finish(mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = self.command_tx.send(TerminalCommand::Shutdown);

        if let Some(handle) = self.subscription_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        if let Some(handle) = self.terminal_handle.take() {
            let _ = tokio::task::spawn_blocking(move || handle.join()).await;
        }
    }
}

async fn drive_runtime_subscription(
    mut subscription: RuntimeSubscription,
    agent: Arc<AgentRuntime>,
    command_tx: mpsc::Sender<TerminalCommand>,
) {
    loop {
        match subscription.recv().await {
            Ok(event) => {
                if command_tx
                    .send(TerminalCommand::RuntimeEvent(Box::new(event.payload)))
                    .is_err()
                {
                    break;
                }
            }
            Err(AgentRuntimeError::StaleRuntime {
                thread_id,
                latest_seq,
                ..
            }) => {
                let replay_cursor = RuntimeCursor::new(thread_id, latest_seq);
                let replay_subscription = agent.subscribe(Some(replay_cursor)).await;
                if let Ok(events) = replay_subscription.replay() {
                    for event in events {
                        if command_tx
                            .send(TerminalCommand::RuntimeEvent(Box::new(event.payload)))
                            .is_err()
                        {
                            return;
                        }
                    }
                    subscription = replay_subscription;
                    continue;
                }

                if !send_snapshot_fallback(&agent, &command_tx).await {
                    break;
                }
                let snapshot = agent.read_snapshot().await;
                subscription = agent.subscribe(latest_snapshot_cursor(&snapshot)).await;
                match subscription.replay() {
                    Ok(events) => {
                        for event in events {
                            if command_tx
                                .send(TerminalCommand::RuntimeEvent(Box::new(event.payload)))
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        let _ = command_tx.send(TerminalCommand::StreamError(error));
                        break;
                    }
                }
            }
            Err(error) => {
                let _ = command_tx.send(TerminalCommand::StreamError(error));
                break;
            }
        }
    }
}

async fn send_snapshot_fallback(
    agent: &AgentRuntime,
    command_tx: &mpsc::Sender<TerminalCommand>,
) -> bool {
    let snapshot = agent.read_snapshot().await;
    command_tx.send(TerminalCommand::Snapshot(snapshot)).is_ok()
}

fn latest_snapshot_cursor(snapshot: &RuntimeSnapshot) -> Option<RuntimeCursor> {
    snapshot
        .threads
        .iter()
        .max_by_key(|thread| thread.last_seq)
        .map(|thread| RuntimeCursor::new(thread.thread_id, thread.last_seq))
}

fn drive_terminal(
    command_rx: mpsc::Receiver<TerminalCommand>,
    coordinator: Arc<UserInputCoordinator>,
    prompt_fn: PromptFn,
    approval_prompt_fn: ApprovalPromptFn,
    handle: tokio::runtime::Handle,
    shutdown: Arc<AtomicBool>,
) {
    let mut renderer = ChatRenderer::default();

    while let Ok(command) = command_rx.recv() {
        match command {
            TerminalCommand::RuntimeEvent(payload) => {
                if renderer.render_payload(*payload) {
                    break;
                }
            }
            TerminalCommand::UserInputRequest(request) => {
                handle_prompt_request(
                    request,
                    coordinator.clone(),
                    prompt_fn.clone(),
                    handle.clone(),
                    shutdown.clone(),
                );
            }
            TerminalCommand::ApprovalRequest {
                request,
                response_tx,
            } => {
                let decision = if shutdown.load(Ordering::Relaxed) {
                    ApprovalDecision::Cancel
                } else {
                    approval_prompt_fn(request)
                };
                let _ = response_tx.send(decision);
            }
            TerminalCommand::Snapshot(snapshot) => renderer.render_snapshot(snapshot),
            TerminalCommand::StreamError(error) => {
                eprintln!("\n[roci] runtime event stream ended: {error:?}");
                break;
            }
            TerminalCommand::Shutdown => break,
        }
    }
}

fn default_approval_prompt_fn() -> ApprovalPromptFn {
    Arc::new(prompt_for_approval)
}

fn prompt_for_approval(request: ApprovalRequest) -> ApprovalDecision {
    eprintln!("\n? approval required: {}", request.id);
    eprintln!("  kind: {:?}", request.kind);
    if let Some(reason) = request.reason.as_deref() {
        eprintln!("  reason: {}", truncate_preview(reason, 200));
    }
    if !request.payload.is_null() {
        eprintln!(
            "  payload: {}",
            truncate_preview(&request.payload.to_string(), 400)
        );
    }
    if let Some(update) = request.suggested_policy_change.as_ref() {
        eprintln!(
            "  suggested policy: rule={:?} argv={:?}",
            update.rule_id, update.argv
        );
    }

    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        eprintln!("  declining: interactive terminal unavailable");
        return ApprovalDecision::Decline;
    }

    loop {
        eprint!("  approve? [y]es/[a] session/[n]o/[c]ancel: ");
        let _ = io::stderr().flush();
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            eprintln!();
            return ApprovalDecision::Decline;
        }

        match input.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => return ApprovalDecision::Accept,
            "a" | "session" | "always" => return ApprovalDecision::AcceptForSession,
            "" | "n" | "no" => return ApprovalDecision::Decline,
            "c" | "cancel" => return ApprovalDecision::Cancel,
            _ => eprintln!("  enter y, a, n, or c"),
        }
    }
}

#[derive(Default)]
struct ChatRenderer {
    printed_text_by_message_id: HashMap<MessageId, String>,
    completed_message_ids: HashSet<MessageId>,
    started_tool_call_ids: HashSet<String>,
    completed_tool_call_ids: HashSet<String>,
}

impl ChatRenderer {
    fn render_snapshot(&mut self, snapshot: RuntimeSnapshot) {
        let mut stdout = std::io::stdout();
        let mut stderr = std::io::stderr();
        for thread in snapshot.threads {
            let Some(target_turn_id) = thread
                .active_turn_id
                .or_else(|| thread.turns.last().map(|turn| turn.turn_id))
            else {
                continue;
            };
            for message in thread.messages {
                if message.turn_id == target_turn_id {
                    self.render_message_snapshot(message, &mut stdout);
                }
            }
            for tool in thread.tools {
                if tool.turn_id != target_turn_id {
                    continue;
                }
                self.render_tool_start(&tool, &mut stderr);
                self.render_tool_update(tool.clone(), &mut stderr);
                self.render_tool_completion(tool, &mut stderr);
            }
        }
    }

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
                self.render_tool_start(&tool, stderr);
            }
            AgentRuntimeEventPayload::ToolUpdated { tool } => {
                self.render_tool_update(tool, stderr);
            }
            AgentRuntimeEventPayload::ToolCompleted { tool } => {
                self.render_tool_completion(tool, stderr);
            }
            AgentRuntimeEventPayload::ApprovalRequired { approval } => {
                let reason = approval
                    .request
                    .reason
                    .as_deref()
                    .unwrap_or("approval required");
                let _ = writeln!(
                    stderr,
                    "\n? approval {}: {}",
                    approval.request.id,
                    truncate_preview(reason, 120)
                );
            }
            AgentRuntimeEventPayload::ApprovalResolved { approval } => {
                let _ = writeln!(
                    stderr,
                    "  approval {}: {:?}",
                    approval.request.id, approval.decision
                );
            }
            AgentRuntimeEventPayload::ApprovalCanceled { approval } => {
                let _ = writeln!(stderr, "  approval {}: canceled", approval.request.id);
            }
            AgentRuntimeEventPayload::ReasoningUpdated { delta, .. } => {
                if !delta.is_empty() {
                    let _ = writeln!(stderr, "\n[reasoning] {}", truncate_preview(&delta, 160));
                }
            }
            AgentRuntimeEventPayload::PlanUpdated { plan } => {
                let _ = writeln!(stderr, "\n[plan] {}", truncate_preview(&plan.plan, 200));
            }
            AgentRuntimeEventPayload::DiffUpdated { diff } => {
                let _ = writeln!(stderr, "\n[diff]\n{}", truncate_preview(&diff.diff, 400));
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
        if matches!(message.status, roci::agent::MessageStatus::Completed)
            && self.completed_message_ids.contains(&message.message_id)
        {
            return;
        }

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
            self.completed_message_ids.insert(message.message_id);
            self.printed_text_by_message_id.remove(&message.message_id);
        }
    }

    fn render_tool_start(&mut self, tool: &ToolExecutionSnapshot, stderr: &mut impl Write) {
        if self.started_tool_call_ids.insert(tool.tool_call_id.clone()) {
            let _ = writeln!(stderr, "\n⚡ {} ({})", tool.tool_name, tool.tool_call_id);
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

    fn render_tool_completion(&mut self, tool: ToolExecutionSnapshot, stderr: &mut impl Write) {
        if self.completed_tool_call_ids.contains(&tool.tool_call_id) {
            return;
        }
        let Some(result) = tool.final_result else {
            return;
        };
        self.completed_tool_call_ids.insert(tool.tool_call_id);
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
    use roci::agent_loop::{ApprovalKind, ToolUpdatePayload};
    use roci::tools::{Answer, UserInputRequest, UserInputRequestId, UserInputResponse};
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

    fn user_input_request() -> UserInputRequest {
        UserInputRequest {
            request_id: UserInputRequestId::new_v4(),
            tool_call_id: "call_1".to_string(),
            questions: vec![roci::tools::Question {
                id: "q1".to_string(),
                text: "Need input".to_string(),
                options: None,
            }],
            timeout_ms: None,
        }
    }

    fn approval_request() -> ApprovalRequest {
        ApprovalRequest {
            id: "approval_1".to_string(),
            kind: ApprovalKind::CommandExecution,
            reason: Some("Run shell".to_string()),
            payload: serde_json::json!({ "tool_name": "shell" }),
            suggested_policy_change: None,
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

    #[tokio::test]
    async fn raw_agent_sink_forwards_user_input_into_terminal_actor() {
        let coordinator = Arc::new(UserInputCoordinator::new());
        let request = user_input_request();
        let pending = coordinator.create_request(request.clone()).await.unwrap();
        let prompt_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let renderer = RuntimeEventRenderer::spawn_with_prompt_fn(
            coordinator.clone(),
            Arc::new({
                let prompt_calls = prompt_calls.clone();
                move |request, _, _, _| {
                    prompt_calls.fetch_add(1, Ordering::Relaxed);
                    Ok(Some(UserInputResponse {
                        request_id: request.request_id,
                        answers: vec![Answer {
                            question_id: "q1".to_string(),
                            content: "yes".to_string(),
                        }],
                        canceled: false,
                    }))
                }
            }),
        );

        let sink = renderer.build_agent_sink();
        sink(AgentEvent::MessageStart {
            message: ModelMessage::assistant("ignore"),
        });
        sink(AgentEvent::UserInputRequested {
            request: request.clone(),
        });

        let response = pending.wait(Some(100)).await.unwrap();
        assert_eq!(response.request_id, request.request_id);
        assert_eq!(prompt_calls.load(Ordering::Relaxed), 1);

        renderer.finish().await;
    }

    #[tokio::test]
    async fn approval_handler_forwards_request_into_terminal_actor() {
        let coordinator = Arc::new(UserInputCoordinator::new());
        let prompt_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let renderer = RuntimeEventRenderer::spawn_with_prompt_fns(
            coordinator,
            Arc::new(|_, _, _, _| Ok(None)),
            Arc::new({
                let prompt_calls = prompt_calls.clone();
                move |request| {
                    prompt_calls.fetch_add(1, Ordering::Relaxed);
                    assert_eq!(request.id, "approval_1");
                    ApprovalDecision::AcceptForSession
                }
            }),
        );

        let handler = renderer.build_approval_handler();
        let decision = handler(approval_request()).await;

        assert_eq!(decision, ApprovalDecision::AcceptForSession);
        assert_eq!(prompt_calls.load(Ordering::Relaxed), 1);

        renderer.finish().await;
    }
}
