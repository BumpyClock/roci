use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use roci::agent::UserInputCoordinator;
use roci::agent_loop::AgentEvent;
use roci::tools::{
    Answer, UserInputError, UserInputRequest, UserInputRequestId, UserInputResponse,
};

pub(crate) type PromptFn = Arc<
    dyn Fn(
            UserInputRequest,
            Arc<UserInputCoordinator>,
            tokio::runtime::Handle,
            Arc<AtomicBool>,
        ) -> Result<Option<UserInputResponse>, UserInputError>
        + Send
        + Sync,
>;

enum PromptCommand {
    Request(UserInputRequest),
    Shutdown,
}

pub(crate) struct PromptHost {
    command_tx: Sender<PromptCommand>,
    shutdown: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl PromptHost {
    pub(crate) fn spawn(coordinator: Arc<UserInputCoordinator>) -> Self {
        Self::spawn_with_prompt_fn(
            coordinator,
            Arc::new(|request, coordinator, handle, shutdown| {
                prompt_user_for_input(&request, &coordinator, &handle, &shutdown)
            }),
        )
    }

    #[cfg(test)]
    pub(crate) fn spawn_with_prompt_fn(
        coordinator: Arc<UserInputCoordinator>,
        prompt_fn: PromptFn,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = shutdown.clone();
        let handle = tokio::runtime::Handle::current();
        let join_handle = std::thread::spawn(move || {
            run_prompt_host_loop(command_rx, coordinator, prompt_fn, handle, thread_shutdown);
        });

        Self {
            command_tx,
            shutdown,
            join_handle: Some(join_handle),
        }
    }

    #[cfg(not(test))]
    fn spawn_with_prompt_fn(coordinator: Arc<UserInputCoordinator>, prompt_fn: PromptFn) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = shutdown.clone();
        let handle = tokio::runtime::Handle::current();
        let join_handle = std::thread::spawn(move || {
            run_prompt_host_loop(command_rx, coordinator, prompt_fn, handle, thread_shutdown);
        });

        Self {
            command_tx,
            shutdown,
            join_handle: Some(join_handle),
        }
    }

    pub(crate) fn build_agent_sink(&self) -> Arc<dyn Fn(AgentEvent) + Send + Sync> {
        let command_tx = self.command_tx.clone();
        Arc::new(move |event: AgentEvent| match event {
            AgentEvent::UserInputRequested { request } => {
                let _ = command_tx.send(PromptCommand::Request(request));
            }
            other => super::render_agent_event(other),
        })
    }

    #[cfg(test)]
    pub(crate) fn enqueue_request(&self, request: UserInputRequest) {
        let _ = self.command_tx.send(PromptCommand::Request(request));
    }

    pub(crate) fn shutdown(mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = self.command_tx.send(PromptCommand::Shutdown);
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
    }
}

fn run_prompt_host_loop(
    command_rx: Receiver<PromptCommand>,
    coordinator: Arc<UserInputCoordinator>,
    prompt_fn: PromptFn,
    handle: tokio::runtime::Handle,
    shutdown: Arc<AtomicBool>,
) {
    while let Ok(command) = command_rx.recv() {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        match command {
            PromptCommand::Request(request) => {
                let fallback_request_id = request.request_id;
                let outcome = prompt_fn(
                    request,
                    coordinator.clone(),
                    handle.clone(),
                    shutdown.clone(),
                );
                match outcome {
                    Ok(Some(response)) => {
                        let _ = handle.block_on(coordinator.submit_response(response));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let request_id = match &error {
                            UserInputError::UnknownRequest { request_id }
                            | UserInputError::Timeout { request_id }
                            | UserInputError::Canceled { request_id }
                            | UserInputError::InteractivePromptUnavailable { request_id, .. } => {
                                *request_id
                            }
                            UserInputError::NoCallback => fallback_request_id,
                        };
                        let _ = handle.block_on(coordinator.submit_error(request_id, error));
                    }
                }
            }
            PromptCommand::Shutdown => break,
        }
    }
}

fn prompt_user_for_input(
    request: &UserInputRequest,
    coordinator: &Arc<UserInputCoordinator>,
    handle: &tokio::runtime::Handle,
    shutdown: &Arc<AtomicBool>,
) -> Result<Option<UserInputResponse>, UserInputError> {
    eprintln!("\n🤖 Agent requests input:");

    ensure_interactive_prompt_available(request.request_id)?;

    let raw_mode =
        RawModeGuard::new().map_err(|error| UserInputError::InteractivePromptUnavailable {
            request_id: request.request_id,
            reason: format!("failed to enable terminal raw mode: {error}"),
        })?;
    drain_terminal_events();
    let mut answers = Vec::new();

    for question in &request.questions {
        eprintln!("\n  {}", question.text);

        let input_prompt = if let Some(options) = &question.options {
            eprintln!("  Options:");
            for (i, opt) in options.iter().enumerate() {
                eprintln!("    [{}] {}", i + 1, opt.label);
            }
            format!("  Enter choice (1-{}): ", options.len())
        } else {
            "  Enter response: ".to_string()
        };

        eprint!("{input_prompt}");

        let _ = io::stderr().flush();

        let Some(input) = prompt_line_cancellable(
            request.request_id,
            coordinator,
            handle,
            shutdown,
            &input_prompt,
        )?
        else {
            return Ok(None);
        };

        if input.is_empty() {
            return Ok(Some(UserInputResponse {
                request_id: request.request_id,
                answers,
                canceled: true,
            }));
        }

        let content = if let Some(options) = &question.options {
            if let Ok(idx) = input.parse::<usize>() {
                if idx > 0 && idx <= options.len() {
                    options[idx - 1].id.clone()
                } else {
                    input
                }
            } else {
                input
            }
        } else {
            input
        };

        answers.push(Answer {
            question_id: question.id.clone(),
            content,
        });
    }

    let _raw_mode = raw_mode;

    Ok(Some(UserInputResponse {
        request_id: request.request_id,
        answers,
        canceled: false,
    }))
}

fn prompt_line_cancellable(
    request_id: UserInputRequestId,
    coordinator: &Arc<UserInputCoordinator>,
    handle: &tokio::runtime::Handle,
    shutdown: &Arc<AtomicBool>,
    prompt: &str,
) -> Result<Option<String>, UserInputError> {
    let mut buffer = String::new();

    loop {
        if shutdown.load(Ordering::Relaxed) || !handle.block_on(coordinator.is_pending(request_id))
        {
            drain_terminal_events();
            eprintln!();
            return Ok(None);
        }

        if !event::poll(Duration::from_millis(50)).map_err(|error| {
            UserInputError::InteractivePromptUnavailable {
                request_id,
                reason: format!("failed to poll terminal input: {error}"),
            }
        })? {
            continue;
        }

        let Event::Key(key) =
            event::read().map_err(|error| UserInputError::InteractivePromptUnavailable {
                request_id,
                reason: format!("failed to read terminal input: {error}"),
            })?
        else {
            continue;
        };

        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Enter => {
                eprintln!();
                return Ok(Some(buffer.trim().to_string()));
            }
            KeyCode::Backspace => {
                buffer.pop();
                redraw_buffer(prompt, &buffer);
            }
            KeyCode::Esc => {
                eprintln!();
                return Ok(Some(String::new()));
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                eprintln!();
                return Ok(Some(String::new()));
            }
            KeyCode::Char(ch)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                buffer.push(ch);
                redraw_buffer(prompt, &buffer);
            }
            _ => {}
        }
    }
}

fn ensure_interactive_prompt_available(
    request_id: UserInputRequestId,
) -> Result<(), UserInputError> {
    if !io::stdin().is_terminal() {
        return Err(UserInputError::InteractivePromptUnavailable {
            request_id,
            reason: "stdin is not an interactive terminal".to_string(),
        });
    }
    if !io::stderr().is_terminal() {
        return Err(UserInputError::InteractivePromptUnavailable {
            request_id,
            reason: "stderr is not an interactive terminal".to_string(),
        });
    }
    Ok(())
}

fn redraw_buffer(prompt: &str, buffer: &str) {
    eprint!("\r\x1b[2K{prompt}{buffer}");
    let _ = io::stderr().flush();
}

fn drain_terminal_events() {
    while matches!(event::poll(Duration::from_millis(0)), Ok(true)) {
        if event::read().is_err() {
            break;
        }
    }
}

struct RawModeGuard;

impl RawModeGuard {
    fn new() -> io::Result<Self> {
        terminal::enable_raw_mode().map_err(io::Error::other)?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_request() -> UserInputRequest {
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

    #[tokio::test]
    async fn prompt_host_submits_response_for_active_request() {
        let coordinator = Arc::new(UserInputCoordinator::new());
        let request = test_request();
        let pending = coordinator.create_request(request.clone()).await.unwrap();
        let host = PromptHost::spawn_with_prompt_fn(
            coordinator.clone(),
            Arc::new(|request, _, _, _| {
                Ok(Some(UserInputResponse {
                    request_id: request.request_id,
                    answers: vec![Answer {
                        question_id: "q1".to_string(),
                        content: "yes".to_string(),
                    }],
                    canceled: false,
                }))
            }),
        );

        host.enqueue_request(request.clone());
        let response = pending.wait(Some(100)).await.unwrap();

        assert_eq!(response.request_id, request.request_id);
        assert_eq!(response.answers[0].content, "yes");

        host.shutdown();
    }

    #[tokio::test]
    async fn prompt_host_ignores_late_response_after_timeout() {
        let coordinator = Arc::new(UserInputCoordinator::new());
        let request = test_request();
        let pending = coordinator.create_request(request.clone()).await.unwrap();
        let release_flag = Arc::new(AtomicBool::new(false));
        let host = PromptHost::spawn_with_prompt_fn(
            coordinator.clone(),
            Arc::new({
                let release_flag = release_flag.clone();
                move |request, _, _, shutdown| {
                    while !shutdown.load(Ordering::Relaxed) {
                        if release_flag.load(Ordering::Relaxed) {
                            return Ok(Some(UserInputResponse {
                                request_id: request.request_id,
                                answers: vec![Answer {
                                    question_id: "q1".to_string(),
                                    content: "late".to_string(),
                                }],
                                canceled: false,
                            }));
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Ok(None)
                }
            }),
        );

        host.enqueue_request(request.clone());
        let result = pending.wait(Some(10)).await;
        assert!(matches!(result, Err(UserInputError::Timeout { .. })));

        release_flag.store(true, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!coordinator.is_pending(request.request_id).await);

        host.shutdown();
    }

    #[tokio::test]
    async fn prompt_host_submits_interactive_unavailable_error() {
        let coordinator = Arc::new(UserInputCoordinator::new());
        let request = test_request();
        let pending = coordinator.create_request(request.clone()).await.unwrap();
        let host = PromptHost::spawn_with_prompt_fn(
            coordinator.clone(),
            Arc::new(|request, _, _, _| {
                Err(UserInputError::InteractivePromptUnavailable {
                    request_id: request.request_id,
                    reason: "stdin is not an interactive terminal".to_string(),
                })
            }),
        );

        host.enqueue_request(request.clone());
        let result = pending.wait(Some(100)).await;
        assert!(matches!(
            result,
            Err(UserInputError::InteractivePromptUnavailable { .. })
        ));

        host.shutdown();
    }
}
