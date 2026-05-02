use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use roci::agent::HumanInteractionCoordinator;
use roci::tools::{
    AskUserChoice, AskUserFormField, AskUserFormInputKind, AskUserPrompt, UserInputError,
    UserInputRequest, UserInputRequestId, UserInputResponse, UserInputResult, UserInputValue,
};

pub(crate) type PromptFn = Arc<
    dyn Fn(
            UserInputRequest,
            Arc<HumanInteractionCoordinator>,
            tokio::runtime::Handle,
            Arc<AtomicBool>,
        ) -> Result<Option<UserInputResponse>, UserInputError>
        + Send
        + Sync,
>;

pub(crate) fn default_prompt_fn() -> PromptFn {
    Arc::new(|request, coordinator, handle, shutdown| {
        prompt_user_for_input(&request, &coordinator, &handle, &shutdown)
    })
}

pub(crate) fn handle_prompt_request(
    request: UserInputRequest,
    coordinator: Arc<HumanInteractionCoordinator>,
    prompt_fn: PromptFn,
    handle: tokio::runtime::Handle,
    shutdown: Arc<AtomicBool>,
) {
    if shutdown.load(Ordering::Relaxed) {
        return;
    }

    let fallback_request_id = request.request_id;
    let outcome = prompt_fn(request, coordinator.clone(), handle.clone(), shutdown);
    match outcome {
        Ok(Some(response)) => {
            let _ = handle.block_on(coordinator.submit_user_input_response(response));
        }
        Ok(None) => {}
        Err(error) => {
            let request_id = match &error {
                UserInputError::UnknownRequest { request_id }
                | UserInputError::Timeout { request_id }
                | UserInputError::Canceled { request_id }
                | UserInputError::InteractivePromptUnavailable { request_id, .. } => *request_id,
                UserInputError::NoCallback => fallback_request_id,
            };
            let _ = handle.block_on(coordinator.submit_user_input_error(request_id, error));
        }
    }
}

fn prompt_user_for_input(
    request: &UserInputRequest,
    coordinator: &Arc<HumanInteractionCoordinator>,
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
    let result = prompt_for_semantic_prompt(
        request.request_id,
        &request.prompt,
        coordinator,
        handle,
        shutdown,
    )?;

    let _raw_mode = raw_mode;

    Ok(Some(UserInputResponse {
        request_id: request.request_id,
        result,
    }))
}

fn prompt_for_semantic_prompt(
    request_id: UserInputRequestId,
    prompt: &AskUserPrompt,
    coordinator: &Arc<HumanInteractionCoordinator>,
    handle: &tokio::runtime::Handle,
    shutdown: &Arc<AtomicBool>,
) -> Result<UserInputResult, UserInputError> {
    match prompt {
        AskUserPrompt::Question {
            question, default, ..
        } => {
            let input = prompt_text(
                request_id,
                question,
                "Enter response",
                coordinator,
                handle,
                shutdown,
            )?;
            match input {
                PromptLine::Input(answer) if answer.is_empty() => Ok(default
                    .clone()
                    .map(|answer| UserInputResult::Question { answer })
                    .unwrap_or(UserInputResult::Canceled)),
                PromptLine::Input(answer) => Ok(UserInputResult::Question { answer }),
                PromptLine::Canceled => Ok(UserInputResult::Canceled),
            }
        }
        AskUserPrompt::Confirm {
            question, default, ..
        } => prompt_confirm(
            request_id,
            question,
            *default,
            coordinator,
            handle,
            shutdown,
        ),
        AskUserPrompt::Choice {
            question,
            choices,
            default,
            ..
        } => prompt_choice(
            request_id,
            question,
            choices,
            default.as_deref(),
            coordinator,
            handle,
            shutdown,
        )
        .map(|choice| match choice {
            Some(choice) => UserInputResult::Choice { choice },
            None => UserInputResult::Canceled,
        }),
        AskUserPrompt::MultiChoice {
            question,
            choices,
            default,
            min_selected,
            max_selected,
            ..
        } => prompt_multi_choice(
            request_id,
            question,
            choices,
            default,
            *min_selected,
            *max_selected,
            coordinator,
            handle,
            shutdown,
        )
        .map(|choices| match choices {
            Some(choices) => UserInputResult::MultiChoice { choices },
            None => UserInputResult::Canceled,
        }),
        AskUserPrompt::Form { title, fields, .. } => {
            if let Some(title) = title {
                eprintln!("\n  {title}");
            }
            let mut values = BTreeMap::new();
            for field in fields {
                let Some(value) =
                    prompt_form_field(request_id, field, coordinator, handle, shutdown)?
                else {
                    return Ok(UserInputResult::Canceled);
                };
                values.insert(field.id.clone(), value);
            }
            Ok(UserInputResult::Form { values })
        }
    }
}

fn prompt_text(
    request_id: UserInputRequestId,
    label: &str,
    action: &str,
    coordinator: &Arc<HumanInteractionCoordinator>,
    handle: &tokio::runtime::Handle,
    shutdown: &Arc<AtomicBool>,
) -> Result<PromptLine, UserInputError> {
    eprintln!("\n  {label}");
    let input_prompt = format!("  {action}: ");
    eprint!("{input_prompt}");
    let _ = io::stderr().flush();
    prompt_line_cancellable(request_id, coordinator, handle, shutdown, &input_prompt)
        .map(|line| line.unwrap_or(PromptLine::Canceled))
}

fn prompt_confirm(
    request_id: UserInputRequestId,
    question: &str,
    default: Option<bool>,
    coordinator: &Arc<HumanInteractionCoordinator>,
    handle: &tokio::runtime::Handle,
    shutdown: &Arc<AtomicBool>,
) -> Result<UserInputResult, UserInputError> {
    loop {
        let action = match default {
            Some(true) => "Confirm [Y/n]",
            Some(false) => "Confirm [y/N]",
            None => "Confirm [y/n]",
        };
        match prompt_text(request_id, question, action, coordinator, handle, shutdown)? {
            PromptLine::Canceled => return Ok(UserInputResult::Canceled),
            PromptLine::Input(input) if input.is_empty() => {
                if let Some(confirmed) = default {
                    return Ok(UserInputResult::Confirm { confirmed });
                }
            }
            PromptLine::Input(input) => match input.to_ascii_lowercase().as_str() {
                "y" | "yes" => return Ok(UserInputResult::Confirm { confirmed: true }),
                "n" | "no" => return Ok(UserInputResult::Confirm { confirmed: false }),
                _ => {}
            },
        }
        eprintln!("  enter y or n");
    }
}

#[allow(clippy::too_many_arguments)]
fn prompt_choice(
    request_id: UserInputRequestId,
    question: &str,
    choices: &[AskUserChoice],
    default: Option<&str>,
    coordinator: &Arc<HumanInteractionCoordinator>,
    handle: &tokio::runtime::Handle,
    shutdown: &Arc<AtomicBool>,
) -> Result<Option<String>, UserInputError> {
    loop {
        print_choices(question, choices);
        let input_prompt = format!("  Enter choice (1-{}): ", choices.len());
        eprint!("{input_prompt}");
        let _ = io::stderr().flush();
        let line =
            prompt_line_cancellable(request_id, coordinator, handle, shutdown, &input_prompt)?;
        let Some(PromptLine::Input(input)) = line else {
            return Ok(None);
        };
        if input.is_empty() {
            return Ok(default.map(str::to_string));
        }
        if let Some(choice) = resolve_choice(&input, choices) {
            return Ok(Some(choice));
        }
        eprintln!("  enter number or choice id");
    }
}

#[allow(clippy::too_many_arguments)]
fn prompt_multi_choice(
    request_id: UserInputRequestId,
    question: &str,
    choices: &[AskUserChoice],
    default: &[String],
    min_selected: Option<usize>,
    max_selected: Option<usize>,
    coordinator: &Arc<HumanInteractionCoordinator>,
    handle: &tokio::runtime::Handle,
    shutdown: &Arc<AtomicBool>,
) -> Result<Option<Vec<String>>, UserInputError> {
    loop {
        print_choices(question, choices);
        let input_prompt = "  Enter choices comma-separated: ";
        eprint!("{input_prompt}");
        let _ = io::stderr().flush();
        let line =
            prompt_line_cancellable(request_id, coordinator, handle, shutdown, input_prompt)?;
        let Some(PromptLine::Input(input)) = line else {
            return Ok(None);
        };
        let selected = if input.is_empty() {
            default.to_vec()
        } else {
            let mut resolved = Vec::new();
            let mut valid = true;
            for item in input
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
            {
                if let Some(choice) = resolve_choice(item, choices) {
                    resolved.push(choice);
                } else {
                    valid = false;
                    break;
                }
            }
            if !valid {
                eprintln!("  enter numbers or choice ids");
                continue;
            }
            resolved
        };
        if min_selected.is_some_and(|min| selected.len() < min) {
            eprintln!("  select at least {}", min_selected.unwrap_or_default());
            continue;
        }
        if max_selected.is_some_and(|max| selected.len() > max) {
            eprintln!("  select at most {}", max_selected.unwrap_or_default());
            continue;
        }
        return Ok(Some(selected));
    }
}

fn prompt_form_field(
    request_id: UserInputRequestId,
    field: &AskUserFormField,
    coordinator: &Arc<HumanInteractionCoordinator>,
    handle: &tokio::runtime::Handle,
    shutdown: &Arc<AtomicBool>,
) -> Result<Option<UserInputValue>, UserInputError> {
    match field.input_kind {
        AskUserFormInputKind::Text => {
            match prompt_text(
                request_id,
                &field.label,
                "Enter value",
                coordinator,
                handle,
                shutdown,
            )? {
                PromptLine::Canceled => Ok(None),
                PromptLine::Input(input) if input.is_empty() => Ok(field.default.clone()),
                PromptLine::Input(input) => Ok(Some(UserInputValue::Text(input))),
            }
        }
        AskUserFormInputKind::Boolean => {
            let result = prompt_confirm(
                request_id,
                &field.label,
                field.default.as_ref().and_then(|value| match value {
                    UserInputValue::Boolean(value) => Some(*value),
                    _ => None,
                }),
                coordinator,
                handle,
                shutdown,
            )?;
            Ok(match result {
                UserInputResult::Confirm { confirmed } => Some(UserInputValue::Boolean(confirmed)),
                _ => None,
            })
        }
        AskUserFormInputKind::Number => loop {
            match prompt_text(
                request_id,
                &field.label,
                "Enter number",
                coordinator,
                handle,
                shutdown,
            )? {
                PromptLine::Canceled => return Ok(None),
                PromptLine::Input(input) if input.is_empty() => return Ok(field.default.clone()),
                PromptLine::Input(input) => match input.parse::<f64>() {
                    Ok(number) => return Ok(Some(UserInputValue::Number(number))),
                    Err(_) => eprintln!("  enter number"),
                },
            }
        },
        AskUserFormInputKind::Choice => prompt_choice(
            request_id,
            &field.label,
            &field.choices,
            field.default.as_ref().and_then(|value| match value {
                UserInputValue::Choice(value) => Some(value.as_str()),
                _ => None,
            }),
            coordinator,
            handle,
            shutdown,
        )
        .map(|choice| choice.map(UserInputValue::Choice)),
        AskUserFormInputKind::MultiChoice => prompt_multi_choice(
            request_id,
            &field.label,
            &field.choices,
            field.default.as_ref().map_or(&[], |value| match value {
                UserInputValue::MultiChoice(values) => values.as_slice(),
                _ => &[],
            }),
            None,
            None,
            coordinator,
            handle,
            shutdown,
        )
        .map(|choices| choices.map(UserInputValue::MultiChoice)),
    }
}

fn print_choices(question: &str, choices: &[AskUserChoice]) {
    eprintln!("\n  {question}");
    eprintln!("  Options:");
    for (index, choice) in choices.iter().enumerate() {
        eprintln!("    [{}] {}", index + 1, choice.label);
    }
}

fn resolve_choice(input: &str, choices: &[AskUserChoice]) -> Option<String> {
    if let Ok(index) = input.parse::<usize>() {
        if (1..=choices.len()).contains(&index) {
            return Some(choices[index - 1].id.clone());
        }
    }
    choices
        .iter()
        .find(|choice| choice.id == input)
        .map(|choice| choice.id.clone())
}

enum PromptLine {
    Input(String),
    Canceled,
}

fn prompt_line_cancellable(
    request_id: UserInputRequestId,
    coordinator: &Arc<HumanInteractionCoordinator>,
    handle: &tokio::runtime::Handle,
    shutdown: &Arc<AtomicBool>,
    prompt: &str,
) -> Result<Option<PromptLine>, UserInputError> {
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
                return Ok(Some(PromptLine::Input(buffer.trim().to_string())));
            }
            KeyCode::Backspace => {
                buffer.pop();
                redraw_buffer(prompt, &buffer);
            }
            KeyCode::Esc => {
                eprintln!();
                return Ok(Some(PromptLine::Canceled));
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                eprintln!();
                return Ok(Some(PromptLine::Canceled));
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
            prompt: AskUserPrompt::Question {
                id: "q1".to_string(),
                question: "Need input".to_string(),
                placeholder: None,
                default: None,
                multiline: false,
            },
            timeout_ms: None,
        }
    }

    fn spawn_prompt_handler(
        request: UserInputRequest,
        coordinator: Arc<HumanInteractionCoordinator>,
        prompt_fn: PromptFn,
        shutdown: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<()> {
        let handle = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            handle_prompt_request(request, coordinator, prompt_fn, handle, shutdown);
        })
    }

    #[tokio::test]
    async fn prompt_handler_submits_response_for_active_request() {
        let coordinator = Arc::new(HumanInteractionCoordinator::new());
        let request = test_request();
        let pending = coordinator
            .create_user_input_request(request.clone())
            .await
            .unwrap();
        let join_handle = spawn_prompt_handler(
            request.clone(),
            coordinator.clone(),
            Arc::new(|request, _, _, _| {
                Ok(Some(UserInputResponse {
                    request_id: request.request_id,
                    result: UserInputResult::Question {
                        answer: "yes".to_string(),
                    },
                }))
            }),
            Arc::new(AtomicBool::new(false)),
        );

        let response = pending.wait_user_input(Some(100)).await.unwrap();

        assert_eq!(response.request_id, request.request_id);
        assert!(matches!(
            response.result,
            UserInputResult::Question { ref answer } if answer == "yes"
        ));
        let _ = join_handle.join();
    }

    #[tokio::test]
    async fn prompt_handler_ignores_late_response_after_timeout() {
        let coordinator = Arc::new(HumanInteractionCoordinator::new());
        let request = test_request();
        let pending = coordinator
            .create_user_input_request(request.clone())
            .await
            .unwrap();
        let release_flag = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = shutdown.clone();
        let join_handle = spawn_prompt_handler(
            request.clone(),
            coordinator.clone(),
            Arc::new({
                let release_flag = release_flag.clone();
                move |request, _, _, shutdown| {
                    while !shutdown.load(Ordering::Relaxed) {
                        if release_flag.load(Ordering::Relaxed) {
                            return Ok(Some(UserInputResponse {
                                request_id: request.request_id,
                                result: UserInputResult::Question {
                                    answer: "late".to_string(),
                                },
                            }));
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Ok(None)
                }
            }),
            thread_shutdown,
        );

        let result = pending.wait_user_input(Some(10)).await;
        assert!(matches!(result, Err(UserInputError::Timeout { .. })));

        release_flag.store(true, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!coordinator.is_pending(request.request_id).await);

        shutdown.store(true, Ordering::Relaxed);
        let _ = join_handle.join();
    }

    #[tokio::test]
    async fn prompt_handler_submits_interactive_unavailable_error() {
        let coordinator = Arc::new(HumanInteractionCoordinator::new());
        let request = test_request();
        let pending = coordinator
            .create_user_input_request(request.clone())
            .await
            .unwrap();
        let join_handle = spawn_prompt_handler(
            request.clone(),
            coordinator.clone(),
            Arc::new(|request, _, _, _| {
                Err(UserInputError::InteractivePromptUnavailable {
                    request_id: request.request_id,
                    reason: "stdin is not an interactive terminal".to_string(),
                })
            }),
            Arc::new(AtomicBool::new(false)),
        );

        let result = pending.wait_user_input(Some(100)).await;
        assert!(matches!(
            result,
            Err(UserInputError::InteractivePromptUnavailable { .. })
        ));
        let _ = join_handle.join();
    }
}
