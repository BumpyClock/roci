//! Child event sink construction for sub-agent supervisor.

use std::sync::Arc;

use tokio::sync::broadcast;

use crate::agent_loop::runner::AgentEventSink;
use crate::agent_loop::AgentEvent;

use super::types::{SubagentEvent, SubagentId};

pub(crate) type CriticalSubagentEventSink = Arc<dyn Fn(SubagentEvent) + Send + Sync>;

pub(crate) fn emit_subagent_event(
    event_tx: &broadcast::Sender<SubagentEvent>,
    critical_sink: Option<&CriticalSubagentEventSink>,
    event: SubagentEvent,
) {
    if let Some(critical_sink) = critical_sink {
        critical_sink(event.clone());
    }
    let _ = event_tx.send(event);
}

/// Build an [`AgentEventSink`] that wraps each child [`AgentEvent`] in a
/// [`SubagentEvent::AgentEvent`] and sends it through the supervisor's event
/// paths.
pub fn build_child_event_sink(
    subagent_id: SubagentId,
    label: Option<String>,
    event_tx: broadcast::Sender<SubagentEvent>,
) -> AgentEventSink {
    build_child_event_sink_with_critical_sink(subagent_id, label, event_tx, None)
}

pub(crate) fn build_child_event_sink_with_critical_sink(
    subagent_id: SubagentId,
    label: Option<String>,
    event_tx: broadcast::Sender<SubagentEvent>,
    critical_sink: Option<CriticalSubagentEventSink>,
) -> AgentEventSink {
    Arc::new(move |event: AgentEvent| {
        if matches!(&event, AgentEvent::MessageUpdate { message, .. } if message.text().is_empty())
        {
            return;
        }
        emit_subagent_event(
            &event_tx,
            critical_sink.as_ref(),
            SubagentEvent::AgentEvent {
                subagent_id,
                label: label.clone(),
                event: Box::new(event),
            },
        );
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use uuid::Uuid;

    #[test]
    fn critical_sink_preserves_more_events_than_broadcast_capacity_in_order() {
        let (tx, mut observer_rx) = broadcast::channel(16);
        let observed = Arc::new(Mutex::new(Vec::new()));
        let critical_observed = observed.clone();
        let critical_sink: CriticalSubagentEventSink = Arc::new(move |event| {
            critical_observed.lock().unwrap().push(event);
        });
        let id = Uuid::new_v4();
        let sink = build_child_event_sink_with_critical_sink(id, None, tx, Some(critical_sink));

        for sequence in 0..300 {
            sink(AgentEvent::ToolExecutionStart {
                tool_call_id: sequence.to_string(),
                tool_name: "stress".into(),
                args: serde_json::Value::Null,
            });
        }

        let observed = observed.lock().unwrap();
        assert_eq!(observed.len(), 300);
        for (sequence, event) in observed.iter().enumerate() {
            assert!(matches!(
                event,
                SubagentEvent::AgentEvent { event, .. }
                    if matches!(event.as_ref(), AgentEvent::ToolExecutionStart { tool_call_id, .. }
                        if tool_call_id == &sequence.to_string())
            ));
        }
        assert!(matches!(
            observer_rx.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
    }

    #[test]
    fn build_child_event_sink_wraps_agent_event() {
        let (tx, mut rx) = broadcast::channel(16);
        let id = Uuid::new_v4();
        let sink = build_child_event_sink(id, Some("test-child".into()), tx);

        let event = AgentEvent::AgentStart {
            run_id: Uuid::new_v4(),
        };
        sink(event);

        let received = rx.try_recv().unwrap();
        match received {
            SubagentEvent::AgentEvent {
                subagent_id,
                label,
                event: boxed_event,
            } => {
                assert_eq!(subagent_id, id);
                assert_eq!(label.as_deref(), Some("test-child"));
                assert!(matches!(*boxed_event, AgentEvent::AgentStart { .. }));
            }
            _ => panic!("expected SubagentEvent::AgentEvent"),
        }
    }

    #[test]
    fn build_child_event_sink_without_label() {
        let (tx, mut rx) = broadcast::channel(16);
        let id = Uuid::new_v4();
        let sink = build_child_event_sink(id, None, tx);

        sink(AgentEvent::AgentStart {
            run_id: Uuid::new_v4(),
        });

        let received = rx.try_recv().unwrap();
        match received {
            SubagentEvent::AgentEvent { label, .. } => {
                assert!(label.is_none());
            }
            _ => panic!("expected SubagentEvent::AgentEvent"),
        }
    }

    #[test]
    fn build_child_event_sink_drops_empty_message_updates() {
        let (tx, mut rx) = broadcast::channel(16);
        let sink = build_child_event_sink(Uuid::new_v4(), None, tx);

        sink(AgentEvent::MessageUpdate {
            message: crate::types::ModelMessage::assistant(""),
            assistant_message_event: crate::types::TextStreamDelta {
                text: String::new(),
                event_type: crate::types::StreamEventType::TextDelta,
                tool_call: None,
                finish_reason: None,
                usage: None,
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            },
        });

        assert!(rx.try_recv().is_err());
    }
}
