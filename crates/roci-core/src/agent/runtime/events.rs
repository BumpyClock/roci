use std::sync::{Arc, Mutex as StdMutex};

use super::{
    AgentRuntime, AgentRuntimeError, AgentRuntimeEvent, AgentSnapshot, AgentState, ChatProjector,
    CollaborationMode, MessageId, TurnId,
};
use crate::agent_loop::approvals::ApprovalDecision;
use crate::agent_loop::runner::AgentEventSink;
use crate::agent_loop::AgentEvent;

#[derive(Debug, Default)]
struct ChatProjectionRunState {
    turn_started: bool,
    active_message_id: Option<MessageId>,
    remaining_context_message_lifecycles: usize,
    skipping_context_message: bool,
    suppress_plan_events: bool,
}

fn ensure_turn_started(
    projector: &mut ChatProjector,
    run_state: &mut ChatProjectionRunState,
    turn_id: TurnId,
) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
    if run_state.turn_started {
        return Ok(Vec::new());
    }

    let event = projector.start_turn(turn_id)?;
    run_state.turn_started = true;
    Ok(vec![event])
}

fn project_agent_event(
    projector: &mut ChatProjector,
    run_state: &mut ChatProjectionRunState,
    turn_id: TurnId,
    event: &AgentEvent,
) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
    let mut events = Vec::new();
    match event {
        AgentEvent::TurnStart { .. } => {
            events.extend(ensure_turn_started(projector, run_state, turn_id)?);
        }
        AgentEvent::MessageStart { message } => {
            if run_state.remaining_context_message_lifecycles > 0 {
                run_state.remaining_context_message_lifecycles -= 1;
                run_state.skipping_context_message = true;
                return Ok(events);
            }
            events.extend(ensure_turn_started(projector, run_state, turn_id)?);
            let projection = projector.start_message(turn_id, message.clone())?;
            run_state.active_message_id = Some(projection.message_id);
            events.push(projection.event);
        }
        AgentEvent::MessageUpdate { message, .. } => {
            events.extend(ensure_turn_started(projector, run_state, turn_id)?);
            let message_id = match run_state.active_message_id {
                Some(message_id) => message_id,
                None => {
                    let projection = projector.start_message(turn_id, message.clone())?;
                    run_state.active_message_id = Some(projection.message_id);
                    events.push(projection.event);
                    projection.message_id
                }
            };
            events.push(projector.update_message(message_id, message.clone())?);
        }
        AgentEvent::MessageEnd { message } => {
            if run_state.skipping_context_message {
                run_state.skipping_context_message = false;
                return Ok(events);
            }
            events.extend(ensure_turn_started(projector, run_state, turn_id)?);
            let message_id = match run_state.active_message_id.take() {
                Some(message_id) => message_id,
                None => {
                    let projection = projector.start_message(turn_id, message.clone())?;
                    events.push(projection.event);
                    projection.message_id
                }
            };
            events.push(projector.complete_message(message_id, message.clone())?);
        }
        AgentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => {
            events.extend(ensure_turn_started(projector, run_state, turn_id)?);
            events.push(projector.start_tool(
                turn_id,
                tool_call_id.clone(),
                tool_name.clone(),
                args.clone(),
            )?);
        }
        AgentEvent::ToolExecutionUpdate {
            tool_call_id,
            tool_name,
            args,
            partial_result,
        } => {
            events.extend(ensure_turn_started(projector, run_state, turn_id)?);
            match projector.update_tool(turn_id, tool_call_id, partial_result.clone()) {
                Ok(event) => events.push(event),
                Err(_) => {
                    events.push(projector.start_tool(
                        turn_id,
                        tool_call_id.clone(),
                        tool_name.clone(),
                        args.clone(),
                    )?);
                    events.push(projector.update_tool(
                        turn_id,
                        tool_call_id,
                        partial_result.clone(),
                    )?);
                }
            }
        }
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            result,
            ..
        } => {
            events.extend(ensure_turn_started(projector, run_state, turn_id)?);
            match projector.complete_tool(turn_id, tool_call_id, result.clone()) {
                Ok(event) => events.push(event),
                Err(_) => {
                    events.push(projector.start_tool(
                        turn_id,
                        tool_call_id.clone(),
                        tool_name.clone(),
                        serde_json::Value::Null,
                    )?);
                    events.push(projector.complete_tool(turn_id, tool_call_id, result.clone())?);
                }
            }
        }
        AgentEvent::Approval { request } => {
            events.extend(ensure_turn_started(projector, run_state, turn_id)?);
            events.push(projector.require_approval(turn_id, request.clone())?);
        }
        AgentEvent::ApprovalResolved {
            request_id,
            decision,
        } => {
            events.extend(ensure_turn_started(projector, run_state, turn_id)?);
            let event = if matches!(decision, ApprovalDecision::Cancel) {
                projector.cancel_approval(turn_id, request_id)?
            } else {
                projector.resolve_approval(turn_id, request_id, *decision)?
            };
            events.push(event);
        }
        AgentEvent::HumanInteractionRequested { request } => {
            events.extend(ensure_turn_started(projector, run_state, turn_id)?);
            events.push(projector.request_human_interaction(turn_id, request.clone())?);
        }
        AgentEvent::HumanInteractionResolved { response } => {
            events.extend(ensure_turn_started(projector, run_state, turn_id)?);
            events.push(projector.resolve_human_interaction(turn_id, response.clone())?);
        }
        AgentEvent::HumanInteractionCanceled { request_id, reason } => {
            events.extend(ensure_turn_started(projector, run_state, turn_id)?);
            events.push(projector.cancel_human_interaction(
                turn_id,
                *request_id,
                reason.clone(),
            )?);
        }
        AgentEvent::Reasoning { text } => {
            events.extend(ensure_turn_started(projector, run_state, turn_id)?);
            events.push(projector.update_reasoning(
                turn_id,
                run_state.active_message_id,
                text.clone(),
            )?);
        }
        AgentEvent::PlanUpdated { plan } => {
            if run_state.suppress_plan_events {
                return Ok(events);
            }
            events.extend(ensure_turn_started(projector, run_state, turn_id)?);
            events.push(projector.update_plan(turn_id, plan.clone())?);
        }
        AgentEvent::DiffUpdated { diff } => {
            events.extend(ensure_turn_started(projector, run_state, turn_id)?);
            events.push(projector.update_diff(turn_id, diff.clone())?);
        }
        AgentEvent::AgentStart { .. }
        | AgentEvent::AgentEnd { .. }
        | AgentEvent::TurnEnd { .. }
        | AgentEvent::Error { .. }
        | AgentEvent::System { .. } => {}
    }

    Ok(events)
}

impl AgentRuntime {
    /// Build an event sink that intercepts [`AgentEvent`]s to update tracking
    /// fields, broadcasts the snapshot, and forwards to the user-provided sink.
    pub(super) fn build_intercepting_sink(
        &self,
        turn_id: TurnId,
        initial_message_count: usize,
        collaboration_mode: CollaborationMode,
    ) -> (AgentEventSink, Arc<StdMutex<Option<AgentRuntimeError>>>) {
        let original_sink = self.config.event_sink.clone();
        let turn_index = self.turn_index.clone();
        let is_streaming = self.is_streaming.clone();
        let messages = self.messages.clone();
        let last_error = self.last_error.clone();
        let state = self.state.clone();
        let snapshot_tx = self.snapshot_tx.clone();
        let chat_projector = self.chat_projector.clone();
        let runtime_event_publish_tx = self.runtime_event_publish_tx.clone();
        let runtime_event_send_lock = self.runtime_event_send_lock.clone();
        let projection_error = Arc::new(StdMutex::new(None));
        let projection_error_for_sink = projection_error.clone();
        let projection_error_for_publish = projection_error.clone();
        let projection_run_state = Arc::new(StdMutex::new(ChatProjectionRunState {
            remaining_context_message_lifecycles: initial_message_count,
            suppress_plan_events: collaboration_mode == CollaborationMode::Plan,
            ..ChatProjectionRunState::default()
        }));

        let sink: AgentEventSink = Arc::new(move |event: AgentEvent| {
            if let (Ok(mut projector), Ok(mut run_state)) =
                (chat_projector.lock(), projection_run_state.lock())
            {
                let projection_result =
                    project_agent_event(&mut projector, &mut run_state, turn_id, &event).and_then(
                        |events| {
                            for event in events {
                                AgentRuntime::queue_runtime_event_to(
                                    &runtime_event_publish_tx,
                                    &runtime_event_send_lock,
                                    event,
                                    projection_error_for_publish.clone(),
                                )?;
                            }
                            Ok(())
                        },
                    );
                if let Err(err) = projection_result {
                    if let Ok(mut stored_error) = projection_error_for_sink.lock() {
                        if stored_error.is_none() {
                            *stored_error = Some(err);
                        }
                    }
                }
            }

            if let AgentEvent::TurnStart {
                turn_index: idx, ..
            } = &event
            {
                if let Ok(mut value) = turn_index.try_lock() {
                    *value = *idx;
                }
                let snapshot = AgentSnapshot {
                    state: state
                        .try_lock()
                        .map(|value| *value)
                        .unwrap_or(AgentState::Running),
                    turn_index: *idx,
                    message_count: messages.try_lock().map(|value| value.len()).unwrap_or(0),
                    is_streaming: is_streaming.try_lock().map(|value| *value).unwrap_or(true),
                    last_error: last_error
                        .try_lock()
                        .map(|value| value.clone())
                        .unwrap_or(None),
                };
                let _ = snapshot_tx.send(snapshot);
            }
            if let Some(ref sink) = original_sink {
                sink(event);
            }
        });

        (sink, projection_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::runtime::chat::{AgentRuntimeEventPayload, ApprovalStatus, ChatProjector};
    use crate::agent_loop::{ApprovalDecision, ApprovalKind, ApprovalRequest};
    use crate::types::ModelMessage;

    fn queued_turn(projector: &mut ChatProjector) -> TurnId {
        projector.queue_turn(Vec::new()).turn_id
    }

    fn approval_request(id: &str) -> ApprovalRequest {
        ApprovalRequest {
            id: id.to_string(),
            kind: ApprovalKind::CommandExecution,
            reason: Some("needs command".to_string()),
            payload: serde_json::json!({ "tool_name": "shell" }),
            suggested_policy_change: None,
        }
    }

    #[test]
    fn projects_approval_events_with_terminal_state() {
        let mut projector = ChatProjector::default();
        let turn_id = queued_turn(&mut projector);
        let mut run_state = ChatProjectionRunState::default();

        let required = project_agent_event(
            &mut projector,
            &mut run_state,
            turn_id,
            &AgentEvent::Approval {
                request: approval_request("approval-1"),
            },
        )
        .expect("approval projects");
        let resolved = project_agent_event(
            &mut projector,
            &mut run_state,
            turn_id,
            &AgentEvent::ApprovalResolved {
                request_id: "approval-1".to_string(),
                decision: ApprovalDecision::Decline,
            },
        )
        .expect("approval resolution projects");
        let thread = projector
            .read_thread(projector.default_thread_id())
            .expect("thread exists");

        assert!(matches!(
            required.last().map(|event| &event.payload),
            Some(AgentRuntimeEventPayload::ApprovalRequired { .. })
        ));
        assert!(matches!(
            resolved.last().map(|event| &event.payload),
            Some(AgentRuntimeEventPayload::ApprovalResolved { .. })
        ));
        assert_eq!(thread.approvals[0].status, ApprovalStatus::Resolved);
        assert_eq!(
            thread.approvals[0].decision,
            Some(ApprovalDecision::Decline)
        );
    }

    #[test]
    fn projects_reasoning_plan_and_diff_agent_events() {
        let mut projector = ChatProjector::default();
        let turn_id = queued_turn(&mut projector);
        let mut run_state = ChatProjectionRunState::default();

        project_agent_event(
            &mut projector,
            &mut run_state,
            turn_id,
            &AgentEvent::MessageStart {
                message: ModelMessage::assistant(""),
            },
        )
        .expect("message starts");
        let reasoning = project_agent_event(
            &mut projector,
            &mut run_state,
            turn_id,
            &AgentEvent::Reasoning {
                text: "analysis".to_string(),
            },
        )
        .expect("reasoning projects");
        let plan = project_agent_event(
            &mut projector,
            &mut run_state,
            turn_id,
            &AgentEvent::PlanUpdated {
                plan: "inspect then edit".to_string(),
            },
        )
        .expect("plan projects");
        let diff = project_agent_event(
            &mut projector,
            &mut run_state,
            turn_id,
            &AgentEvent::DiffUpdated {
                diff: "+line".to_string(),
            },
        )
        .expect("diff projects");
        let thread = projector
            .read_thread(projector.default_thread_id())
            .expect("thread exists");

        assert_eq!(thread.reasoning[0].text, "analysis");
        assert_eq!(thread.reasoning[0].message_id, run_state.active_message_id);
        assert_eq!(thread.plans[0].plan, "inspect then edit");
        assert_eq!(thread.diffs[0].diff, "+line");
        assert!(matches!(
            reasoning.last().map(|event| &event.payload),
            Some(AgentRuntimeEventPayload::ReasoningUpdated { .. })
        ));
        assert!(matches!(
            plan.last().map(|event| &event.payload),
            Some(AgentRuntimeEventPayload::PlanUpdated { .. })
        ));
        assert!(matches!(
            diff.last().map(|event| &event.payload),
            Some(AgentRuntimeEventPayload::DiffUpdated { .. })
        ));
    }

    #[test]
    fn suppresses_plan_agent_events_when_configured() {
        let mut projector = ChatProjector::default();
        let turn_id = queued_turn(&mut projector);
        let mut run_state = ChatProjectionRunState {
            suppress_plan_events: true,
            ..ChatProjectionRunState::default()
        };

        let plan = project_agent_event(
            &mut projector,
            &mut run_state,
            turn_id,
            &AgentEvent::PlanUpdated {
                plan: "inspect then edit".to_string(),
            },
        )
        .expect("plan suppression should project");
        let thread = projector
            .read_thread(projector.default_thread_id())
            .expect("thread exists");

        assert!(plan.is_empty());
        assert!(thread.plans.is_empty());
    }
}
