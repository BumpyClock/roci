//! Child event sink construction for sub-agent supervisor.

use std::sync::Arc;

use tokio::sync::broadcast;

use crate::agent_loop::runner::AgentEventSink;
use crate::agent_loop::AgentEvent;

use super::types::{SubagentEvent, SubagentId};

/// Build an [`AgentEventSink`] that wraps each child [`AgentEvent`] in a
/// [`SubagentEvent::AgentEvent`] and sends it through the supervisor's
/// broadcast channel.
pub fn build_child_event_sink(
    subagent_id: SubagentId,
    label: Option<String>,
    event_tx: broadcast::Sender<SubagentEvent>,
) -> AgentEventSink {
    Arc::new(move |event: AgentEvent| {
        let _ = event_tx.send(SubagentEvent::AgentEvent {
            subagent_id,
            label: label.clone(),
            event: Box::new(event),
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

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
}
