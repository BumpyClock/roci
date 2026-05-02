use std::collections::{HashMap, VecDeque};

use chrono::Utc;

use super::{
    AgentRuntimeError, AgentRuntimeEvent, AgentRuntimeEventPayload, ApprovalSnapshot,
    ApprovalStatus, ChatRuntimeConfig, DiffSnapshot, HumanInteractionSnapshot,
    HumanInteractionStatus, MessageId, MessageSnapshot, MessageStatus, PlanSnapshot,
    ReasoningSnapshot, RuntimeCursor, RuntimeSnapshot, ThreadId, ThreadSnapshot,
    ToolExecutionSnapshot, ToolStatus, TurnId, TurnSnapshot, TurnStatus,
    AGENT_RUNTIME_EVENT_SCHEMA_VERSION,
};
use crate::agent_loop::{ApprovalDecision, ApprovalRequest, ToolUpdatePayload};
use crate::human_interaction::{
    HumanInteractionRequest, HumanInteractionRequestId, HumanInteractionResponse,
};
use crate::types::{AgentToolResult, ModelMessage};

pub type ModelMessages = Vec<ModelMessage>;

#[derive(Debug, Clone)]
pub struct TurnProjection {
    pub turn_id: TurnId,
    pub events: Vec<AgentRuntimeEvent>,
}

#[derive(Debug, Clone)]
pub struct MessageProjection {
    pub message_id: MessageId,
    pub event: AgentRuntimeEvent,
}

#[derive(Debug, Clone)]
pub(crate) struct ThreadState {
    snapshot: ThreadSnapshot,
    events: VecDeque<AgentRuntimeEvent>,
    replay_capacity: usize,
    next_turn_ordinal: u64,
    next_message_ordinal: u64,
}

impl ThreadState {
    #[must_use]
    pub fn new(thread_id: ThreadId, replay_capacity: usize) -> Self {
        Self {
            snapshot: ThreadSnapshot {
                thread_id,
                revision: 0,
                last_seq: 0,
                active_turn_id: None,
                turns: Vec::new(),
                messages: Vec::new(),
                tools: Vec::new(),
                approvals: Vec::new(),
                human_interactions: Vec::new(),
                reasoning: Vec::new(),
                plans: Vec::new(),
                diffs: Vec::new(),
            },
            events: VecDeque::new(),
            replay_capacity,
            next_turn_ordinal: 1,
            next_message_ordinal: 1,
        }
    }

    #[must_use]
    pub fn from_snapshot(snapshot: ThreadSnapshot, replay_capacity: usize) -> Self {
        let next_turn_ordinal = snapshot
            .turns
            .iter()
            .filter(|turn| turn.turn_id.thread_id() == snapshot.thread_id)
            .map(|turn| turn.turn_id.ordinal())
            .max()
            .unwrap_or(0)
            + 1;
        let next_message_ordinal = snapshot
            .messages
            .iter()
            .filter(|message| message.message_id.thread_id() == snapshot.thread_id)
            .map(|message| message.message_id.ordinal())
            .max()
            .unwrap_or(0)
            + 1;

        Self {
            snapshot,
            events: VecDeque::new(),
            replay_capacity,
            next_turn_ordinal,
            next_message_ordinal,
        }
    }

    #[must_use]
    pub fn thread_id(&self) -> ThreadId {
        self.snapshot.thread_id
    }

    #[must_use]
    pub fn read_snapshot(&self) -> ThreadSnapshot {
        self.snapshot.clone()
    }

    fn bootstrap_thread(&mut self, messages: ModelMessages) -> ThreadSnapshot {
        self.snapshot.revision += 1;
        self.snapshot.last_seq += 1;
        self.snapshot.active_turn_id = None;
        self.snapshot.turns.clear();
        self.snapshot.messages.clear();
        self.snapshot.tools.clear();
        self.snapshot.approvals.clear();
        self.snapshot.human_interactions.clear();
        self.snapshot.reasoning.clear();
        self.snapshot.plans.clear();
        self.snapshot.diffs.clear();
        self.events.clear();
        self.next_turn_ordinal = 1;
        self.next_message_ordinal = 1;

        if messages.is_empty() {
            return self.read_snapshot();
        }

        let thread_id = self.thread_id();
        let revision = self.snapshot.revision;
        let imported_at = Utc::now();
        let turn_id = TurnId::new(thread_id, revision, self.next_turn_ordinal);
        self.next_turn_ordinal += 1;

        let mut message_ids = Vec::with_capacity(messages.len());
        let mut snapshots = Vec::with_capacity(messages.len());
        for payload in messages {
            let message_id = self.next_message_id();
            message_ids.push(message_id);
            snapshots.push(MessageSnapshot {
                message_id,
                thread_id,
                turn_id,
                status: MessageStatus::Completed,
                payload,
                created_at: imported_at,
                completed_at: Some(imported_at),
            });
        }

        self.snapshot.turns.push(TurnSnapshot {
            turn_id,
            thread_id,
            status: TurnStatus::Completed,
            message_ids,
            active_tool_call_ids: Vec::new(),
            error: None,
            queued_at: imported_at,
            started_at: Some(imported_at),
            completed_at: Some(imported_at),
        });
        self.snapshot.messages = snapshots;

        self.read_snapshot()
    }

    pub fn events_after(
        &self,
        cursor: RuntimeCursor,
    ) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
        if cursor.thread_id != self.thread_id() {
            return Err(AgentRuntimeError::ThreadNotFound {
                thread_id: cursor.thread_id,
            });
        }

        let oldest_available_seq = self
            .events
            .front()
            .map_or(self.snapshot.last_seq + 1, |event| event.seq);
        if cursor.seq < oldest_available_seq.saturating_sub(1) {
            return Err(AgentRuntimeError::StaleRuntime {
                thread_id: self.thread_id(),
                requested_seq: cursor.seq,
                oldest_available_seq,
                latest_seq: self.snapshot.last_seq,
            });
        }

        Ok(self
            .events
            .iter()
            .filter(|event| event.seq > cursor.seq)
            .cloned()
            .collect())
    }

    fn queue_turn(&mut self, input: ModelMessages) -> TurnProjection {
        let turn_id = TurnId::new(
            self.thread_id(),
            self.snapshot.revision,
            self.next_turn_ordinal,
        );
        self.next_turn_ordinal += 1;

        let queued_at = Utc::now();
        let turn = TurnSnapshot {
            turn_id,
            thread_id: self.thread_id(),
            status: TurnStatus::Queued,
            message_ids: Vec::new(),
            active_tool_call_ids: Vec::new(),
            error: None,
            queued_at,
            started_at: None,
            completed_at: None,
        };
        self.snapshot.turns.push(turn);
        if self.snapshot.active_turn_id.is_none() {
            self.snapshot.active_turn_id = Some(turn_id);
        }

        let mut events = vec![self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::TurnQueued {
                turn: self
                    .turn_snapshot(turn_id)
                    .expect("queued turn was inserted"),
            },
        )];

        for message in input {
            let message_id = self.next_message_id();
            let created_at = Utc::now();
            let started = MessageSnapshot {
                message_id,
                thread_id: self.thread_id(),
                turn_id,
                status: MessageStatus::Streaming,
                payload: message,
                created_at,
                completed_at: None,
            };
            self.snapshot.messages.push(started);
            self.turn_mut(turn_id)
                .expect("queued turn exists")
                .message_ids
                .push(message_id);

            events.push(
                self.event(
                    Some(turn_id),
                    AgentRuntimeEventPayload::MessageStarted {
                        message: self
                            .message_snapshot(message_id)
                            .expect("message was inserted"),
                    },
                ),
            );

            let message = self.message_mut(message_id).expect("message exists");
            message.status = MessageStatus::Completed;
            message.completed_at = Some(Utc::now());
            events.push(
                self.event(
                    Some(turn_id),
                    AgentRuntimeEventPayload::MessageCompleted {
                        message: self
                            .message_snapshot(message_id)
                            .expect("message was completed"),
                    },
                ),
            );
        }

        TurnProjection { turn_id, events }
    }

    fn start_turn(&mut self, turn_id: TurnId) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        let turn = self.turn_mut_or_err(turn_id)?;
        if is_terminal(turn.status) {
            return Err(AgentRuntimeError::AlreadyTerminal {
                turn_id,
                status: turn.status,
            });
        }
        if turn.status == TurnStatus::Running {
            return Err(AgentRuntimeError::ProjectionFailed {
                message: format!("turn already running: {turn_id}"),
            });
        }

        turn.status = TurnStatus::Running;
        turn.started_at = Some(Utc::now());
        self.snapshot.active_turn_id = Some(turn_id);
        Ok(self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::TurnStarted {
                turn: self.turn_snapshot(turn_id).expect("turn was started"),
            },
        ))
    }

    fn complete_turn(&mut self, turn_id: TurnId) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.terminal_turn(turn_id, TurnStatus::Completed, None)
    }

    fn fail_turn(
        &mut self,
        turn_id: TurnId,
        error: impl Into<String>,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.terminal_turn(turn_id, TurnStatus::Failed, Some(error.into()))
    }

    fn cancel_turn(&mut self, turn_id: TurnId) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.terminal_turn(turn_id, TurnStatus::Canceled, None)
    }

    fn start_message(
        &mut self,
        turn_id: TurnId,
        payload: ModelMessage,
    ) -> Result<MessageProjection, AgentRuntimeError> {
        self.ensure_turn_can_project(turn_id)?;
        let message_id = self.next_message_id();
        let message = MessageSnapshot {
            message_id,
            thread_id: self.thread_id(),
            turn_id,
            status: MessageStatus::Streaming,
            payload,
            created_at: Utc::now(),
            completed_at: None,
        };
        self.snapshot.messages.push(message);
        self.turn_mut_or_err(turn_id)?.message_ids.push(message_id);

        let event = self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::MessageStarted {
                message: self
                    .message_snapshot(message_id)
                    .expect("message was inserted"),
            },
        );
        Ok(MessageProjection { message_id, event })
    }

    fn update_message(
        &mut self,
        message_id: MessageId,
        payload: ModelMessage,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        let turn_id = self.message_snapshot_or_err(message_id)?.turn_id;
        self.ensure_turn_can_project(turn_id)?;
        let message = self.message_mut_or_err(message_id)?;
        message.payload = payload;
        Ok(self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::MessageUpdated {
                message: self
                    .message_snapshot(message_id)
                    .expect("message was updated"),
            },
        ))
    }

    fn complete_message(
        &mut self,
        message_id: MessageId,
        payload: ModelMessage,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        let turn_id = self.message_snapshot_or_err(message_id)?.turn_id;
        self.ensure_turn_can_project(turn_id)?;
        let message = self.message_mut_or_err(message_id)?;
        message.payload = payload;
        message.status = MessageStatus::Completed;
        message.completed_at = Some(Utc::now());
        Ok(self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::MessageCompleted {
                message: self
                    .message_snapshot(message_id)
                    .expect("message was completed"),
            },
        ))
    }

    fn start_tool(
        &mut self,
        turn_id: TurnId,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        args: serde_json::Value,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.ensure_turn_can_project(turn_id)?;
        let tool_call_id = tool_call_id.into();
        if self
            .snapshot
            .tools
            .iter()
            .any(|tool| tool.turn_id == turn_id && tool.tool_call_id == tool_call_id)
        {
            return Err(AgentRuntimeError::ProjectionFailed {
                message: format!("tool already started: {tool_call_id}"),
            });
        }

        let tool = ToolExecutionSnapshot {
            tool_call_id: tool_call_id.clone(),
            thread_id: self.thread_id(),
            turn_id,
            tool_name: tool_name.into(),
            args,
            status: ToolStatus::Running,
            partial_result: None,
            final_result: None,
            started_at: Utc::now(),
            completed_at: None,
        };
        self.snapshot.tools.push(tool);
        self.turn_mut_or_err(turn_id)?
            .active_tool_call_ids
            .push(tool_call_id.clone());

        Ok(self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::ToolStarted {
                tool: self
                    .tool_snapshot(turn_id, &tool_call_id)
                    .expect("tool was inserted"),
            },
        ))
    }

    fn update_tool(
        &mut self,
        turn_id: TurnId,
        tool_call_id: &str,
        partial_result: ToolUpdatePayload,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.ensure_turn_can_project(turn_id)?;
        let tool = self.tool_mut_or_err(turn_id, tool_call_id)?;
        tool.partial_result = Some(partial_result);
        Ok(self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::ToolUpdated {
                tool: self
                    .tool_snapshot(turn_id, tool_call_id)
                    .expect("tool was updated"),
            },
        ))
    }

    fn complete_tool(
        &mut self,
        turn_id: TurnId,
        tool_call_id: &str,
        final_result: AgentToolResult,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.ensure_turn_can_project(turn_id)?;
        let tool = self.tool_mut_or_err(turn_id, tool_call_id)?;
        tool.status = ToolStatus::Completed;
        tool.final_result = Some(final_result);
        tool.completed_at = Some(Utc::now());
        if let Some(turn) = self.turn_mut(turn_id) {
            turn.active_tool_call_ids.retain(|id| id != tool_call_id);
        }

        Ok(self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::ToolCompleted {
                tool: self
                    .tool_snapshot(turn_id, tool_call_id)
                    .expect("tool was completed"),
            },
        ))
    }

    fn require_approval(
        &mut self,
        turn_id: TurnId,
        request: ApprovalRequest,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.ensure_turn_can_project(turn_id)?;
        if self
            .snapshot
            .approvals
            .iter()
            .any(|approval| approval.turn_id == turn_id && approval.request.id == request.id)
        {
            return Err(AgentRuntimeError::ProjectionFailed {
                message: format!("approval already requested: {}", request.id),
            });
        }

        self.snapshot.approvals.push(ApprovalSnapshot {
            request: request.clone(),
            thread_id: self.thread_id(),
            turn_id,
            status: ApprovalStatus::Pending,
            decision: None,
            requested_at: Utc::now(),
            resolved_at: None,
        });

        Ok(self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::ApprovalRequired {
                approval: self
                    .approval_snapshot(turn_id, &request.id)
                    .expect("approval was inserted"),
            },
        ))
    }

    fn resolve_approval(
        &mut self,
        turn_id: TurnId,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.ensure_turn_can_project(turn_id)?;
        let approval = self.approval_mut_or_err(turn_id, request_id)?;
        approval.status = ApprovalStatus::Resolved;
        approval.decision = Some(decision);
        approval.resolved_at = Some(Utc::now());

        Ok(self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::ApprovalResolved {
                approval: self
                    .approval_snapshot(turn_id, request_id)
                    .expect("approval was resolved"),
            },
        ))
    }

    fn cancel_approval(
        &mut self,
        turn_id: TurnId,
        request_id: &str,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.ensure_turn_can_project(turn_id)?;
        let approval = self.approval_mut_or_err(turn_id, request_id)?;
        approval.status = ApprovalStatus::Canceled;
        approval.decision = Some(ApprovalDecision::Cancel);
        approval.resolved_at = Some(Utc::now());

        Ok(self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::ApprovalCanceled {
                approval: self
                    .approval_snapshot(turn_id, request_id)
                    .expect("approval was canceled"),
            },
        ))
    }

    fn cancel_pending_approvals(
        &mut self,
        turn_id: TurnId,
    ) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
        self.ensure_turn_can_project(turn_id)?;
        let request_ids = self
            .snapshot
            .approvals
            .iter()
            .filter(|approval| {
                approval.turn_id == turn_id && approval.status == ApprovalStatus::Pending
            })
            .map(|approval| approval.request.id.clone())
            .collect::<Vec<_>>();

        let mut events = Vec::with_capacity(request_ids.len());
        for request_id in request_ids {
            events.push(self.cancel_approval(turn_id, &request_id)?);
        }
        Ok(events)
    }

    fn request_human_interaction(
        &mut self,
        turn_id: TurnId,
        request: HumanInteractionRequest,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.ensure_turn_can_project(turn_id)?;
        if self.snapshot.human_interactions.iter().any(|interaction| {
            interaction.turn_id == turn_id && interaction.request.request_id == request.request_id
        }) {
            return Err(AgentRuntimeError::ProjectionFailed {
                message: format!(
                    "human interaction already requested: {}",
                    request.request_id
                ),
            });
        }

        self.snapshot
            .human_interactions
            .push(HumanInteractionSnapshot {
                request: request.clone(),
                thread_id: self.thread_id(),
                turn_id,
                status: HumanInteractionStatus::Pending,
                response: None,
                error: None,
                requested_at: Utc::now(),
                resolved_at: None,
            });

        Ok(self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::HumanInteractionRequested {
                interaction: self
                    .human_interaction_snapshot(turn_id, request.request_id)
                    .expect("human interaction was inserted"),
            },
        ))
    }

    fn resolve_human_interaction(
        &mut self,
        turn_id: TurnId,
        response: HumanInteractionResponse,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.ensure_turn_can_project(turn_id)?;
        let request_id = response.request_id;
        let interaction = self.human_interaction_mut_or_err(turn_id, request_id)?;
        interaction.status = HumanInteractionStatus::Resolved;
        interaction.response = Some(response);
        interaction.error = None;
        interaction.resolved_at = Some(Utc::now());

        Ok(self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::HumanInteractionResolved {
                interaction: self
                    .human_interaction_snapshot(turn_id, request_id)
                    .expect("human interaction was resolved"),
            },
        ))
    }

    fn cancel_human_interaction(
        &mut self,
        turn_id: TurnId,
        request_id: HumanInteractionRequestId,
        reason: Option<String>,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.ensure_turn_can_project(turn_id)?;
        let interaction = self.human_interaction_mut_or_err(turn_id, request_id)?;
        interaction.status = HumanInteractionStatus::Canceled;
        interaction.error = reason;
        interaction.resolved_at = Some(Utc::now());

        Ok(self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::HumanInteractionCanceled {
                interaction: self
                    .human_interaction_snapshot(turn_id, request_id)
                    .expect("human interaction was canceled"),
            },
        ))
    }

    fn cancel_pending_human_interactions(
        &mut self,
        turn_id: TurnId,
    ) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
        self.ensure_turn_can_project(turn_id)?;
        let request_ids = self
            .snapshot
            .human_interactions
            .iter()
            .filter(|interaction| {
                interaction.turn_id == turn_id
                    && interaction.status == HumanInteractionStatus::Pending
            })
            .map(|interaction| interaction.request.request_id)
            .collect::<Vec<_>>();

        let mut events = Vec::with_capacity(request_ids.len());
        for request_id in request_ids {
            events.push(self.cancel_human_interaction(turn_id, request_id, None)?);
        }
        Ok(events)
    }

    fn update_reasoning(
        &mut self,
        turn_id: TurnId,
        message_id: Option<MessageId>,
        delta: impl Into<String>,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.ensure_turn_can_project(turn_id)?;
        if let Some(message_id) = message_id {
            let message = self.message_snapshot_or_err(message_id)?;
            if message.turn_id != turn_id {
                return Err(AgentRuntimeError::ProjectionFailed {
                    message: format!("message {message_id} does not belong to turn {turn_id}"),
                });
            }
        }

        let delta = delta.into();
        let updated_at = Utc::now();
        if let Some(reasoning) =
            self.snapshot.reasoning.iter_mut().find(|reasoning| {
                reasoning.turn_id == turn_id && reasoning.message_id == message_id
            })
        {
            reasoning.text.push_str(&delta);
            reasoning.updated_at = updated_at;
        } else {
            self.snapshot.reasoning.push(ReasoningSnapshot {
                thread_id: self.thread_id(),
                turn_id,
                message_id,
                text: delta.clone(),
                updated_at,
            });
        }

        Ok(self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::ReasoningUpdated {
                reasoning: self
                    .reasoning_snapshot(turn_id, message_id)
                    .expect("reasoning was updated"),
                delta,
            },
        ))
    }

    fn update_plan(
        &mut self,
        turn_id: TurnId,
        plan: impl Into<String>,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.ensure_turn_can_project(turn_id)?;
        let plan = plan.into();
        let updated_at = Utc::now();
        if let Some(snapshot) = self
            .snapshot
            .plans
            .iter_mut()
            .find(|snapshot| snapshot.turn_id == turn_id)
        {
            snapshot.plan = plan;
            snapshot.updated_at = updated_at;
        } else {
            self.snapshot.plans.push(PlanSnapshot {
                thread_id: self.thread_id(),
                turn_id,
                plan,
                updated_at,
            });
        }

        Ok(self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::PlanUpdated {
                plan: self.plan_snapshot(turn_id).expect("plan was updated"),
            },
        ))
    }

    fn update_diff(
        &mut self,
        turn_id: TurnId,
        diff: impl Into<String>,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.ensure_turn_can_project(turn_id)?;
        let diff = diff.into();
        let updated_at = Utc::now();
        if let Some(snapshot) = self
            .snapshot
            .diffs
            .iter_mut()
            .find(|snapshot| snapshot.turn_id == turn_id)
        {
            snapshot.diff = diff;
            snapshot.updated_at = updated_at;
        } else {
            self.snapshot.diffs.push(DiffSnapshot {
                thread_id: self.thread_id(),
                turn_id,
                diff,
                updated_at,
            });
        }

        Ok(self.event(
            Some(turn_id),
            AgentRuntimeEventPayload::DiffUpdated {
                diff: self.diff_snapshot(turn_id).expect("diff was updated"),
            },
        ))
    }

    fn terminal_turn(
        &mut self,
        turn_id: TurnId,
        status: TurnStatus,
        error: Option<String>,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        let turn = self.turn_mut_or_err(turn_id)?;
        if is_terminal(turn.status) {
            return Err(AgentRuntimeError::AlreadyTerminal {
                turn_id,
                status: turn.status,
            });
        }

        turn.status = status;
        turn.completed_at = Some(Utc::now());
        turn.error = error.clone();
        if self.snapshot.active_turn_id == Some(turn_id) {
            self.snapshot.active_turn_id = None;
        }
        if status == TurnStatus::Canceled {
            self.cancel_pending_approval_snapshots(turn_id);
        }

        let turn = self.turn_snapshot(turn_id).expect("turn was terminal");
        let payload = match status {
            TurnStatus::Completed => AgentRuntimeEventPayload::TurnCompleted { turn },
            TurnStatus::Failed => AgentRuntimeEventPayload::TurnFailed {
                turn,
                error: error.expect("failed turn carries error"),
            },
            TurnStatus::Canceled => AgentRuntimeEventPayload::TurnCanceled { turn },
            TurnStatus::Queued | TurnStatus::Running => {
                return Err(AgentRuntimeError::ProjectionFailed {
                    message: format!("non-terminal status requested: {status:?}"),
                });
            }
        };

        Ok(self.event(Some(turn_id), payload))
    }

    fn event(
        &mut self,
        turn_id: Option<TurnId>,
        payload: AgentRuntimeEventPayload,
    ) -> AgentRuntimeEvent {
        self.snapshot.last_seq += 1;
        let event =
            AgentRuntimeEvent::new(self.snapshot.last_seq, self.thread_id(), turn_id, payload);
        if self.replay_capacity > 0 {
            self.events.push_back(event.clone());
            while self.events.len() > self.replay_capacity {
                self.events.pop_front();
            }
        }
        event
    }

    fn next_message_id(&mut self) -> MessageId {
        let message_id = MessageId::new(
            self.thread_id(),
            self.snapshot.revision,
            self.next_message_ordinal,
        );
        self.next_message_ordinal += 1;
        message_id
    }

    fn ensure_turn_can_project(&self, turn_id: TurnId) -> Result<(), AgentRuntimeError> {
        let turn = self.turn_snapshot_or_err(turn_id)?;
        if is_terminal(turn.status) {
            return Err(AgentRuntimeError::AlreadyTerminal {
                turn_id,
                status: turn.status,
            });
        }
        Ok(())
    }

    fn turn_snapshot(&self, turn_id: TurnId) -> Option<TurnSnapshot> {
        self.snapshot
            .turns
            .iter()
            .find(|turn| turn.turn_id == turn_id)
            .cloned()
    }

    fn turn_snapshot_or_err(&self, turn_id: TurnId) -> Result<TurnSnapshot, AgentRuntimeError> {
        self.ensure_current_turn_revision(turn_id)?;
        self.turn_snapshot(turn_id)
            .ok_or(AgentRuntimeError::TurnNotFound { turn_id })
    }

    fn turn_mut(&mut self, turn_id: TurnId) -> Option<&mut TurnSnapshot> {
        self.snapshot
            .turns
            .iter_mut()
            .find(|turn| turn.turn_id == turn_id)
    }

    fn turn_mut_or_err(&mut self, turn_id: TurnId) -> Result<&mut TurnSnapshot, AgentRuntimeError> {
        self.ensure_current_turn_revision(turn_id)?;
        self.turn_mut(turn_id)
            .ok_or(AgentRuntimeError::TurnNotFound { turn_id })
    }

    fn ensure_current_turn_revision(&self, turn_id: TurnId) -> Result<(), AgentRuntimeError> {
        if turn_id.revision() == self.snapshot.revision {
            return Ok(());
        }

        Err(AgentRuntimeError::StaleRuntime {
            thread_id: self.thread_id(),
            requested_seq: turn_id.revision(),
            oldest_available_seq: self.snapshot.revision,
            latest_seq: self.snapshot.revision,
        })
    }

    fn message_snapshot(&self, message_id: MessageId) -> Option<MessageSnapshot> {
        self.snapshot
            .messages
            .iter()
            .find(|message| message.message_id == message_id)
            .cloned()
    }

    fn message_snapshot_or_err(
        &self,
        message_id: MessageId,
    ) -> Result<MessageSnapshot, AgentRuntimeError> {
        self.message_snapshot(message_id)
            .ok_or(AgentRuntimeError::ProjectionFailed {
                message: format!("message not found: {message_id}"),
            })
    }

    fn message_mut(&mut self, message_id: MessageId) -> Option<&mut MessageSnapshot> {
        self.snapshot
            .messages
            .iter_mut()
            .find(|message| message.message_id == message_id)
    }

    fn message_mut_or_err(
        &mut self,
        message_id: MessageId,
    ) -> Result<&mut MessageSnapshot, AgentRuntimeError> {
        self.message_mut(message_id)
            .ok_or(AgentRuntimeError::ProjectionFailed {
                message: format!("message not found: {message_id}"),
            })
    }

    fn tool_snapshot(&self, turn_id: TurnId, tool_call_id: &str) -> Option<ToolExecutionSnapshot> {
        self.snapshot
            .tools
            .iter()
            .find(|tool| tool.turn_id == turn_id && tool.tool_call_id == tool_call_id)
            .cloned()
    }

    fn tool_mut_or_err(
        &mut self,
        turn_id: TurnId,
        tool_call_id: &str,
    ) -> Result<&mut ToolExecutionSnapshot, AgentRuntimeError> {
        self.snapshot
            .tools
            .iter_mut()
            .find(|tool| tool.turn_id == turn_id && tool.tool_call_id == tool_call_id)
            .ok_or_else(|| AgentRuntimeError::ProjectionFailed {
                message: format!("tool not found: {tool_call_id}"),
            })
    }

    fn approval_snapshot(&self, turn_id: TurnId, request_id: &str) -> Option<ApprovalSnapshot> {
        self.snapshot
            .approvals
            .iter()
            .find(|approval| approval.turn_id == turn_id && approval.request.id == request_id)
            .cloned()
    }

    fn approval_mut_or_err(
        &mut self,
        turn_id: TurnId,
        request_id: &str,
    ) -> Result<&mut ApprovalSnapshot, AgentRuntimeError> {
        self.snapshot
            .approvals
            .iter_mut()
            .find(|approval| approval.turn_id == turn_id && approval.request.id == request_id)
            .ok_or_else(|| AgentRuntimeError::ProjectionFailed {
                message: format!("approval not found: {request_id}"),
            })
    }

    fn human_interaction_snapshot(
        &self,
        turn_id: TurnId,
        request_id: HumanInteractionRequestId,
    ) -> Option<HumanInteractionSnapshot> {
        self.snapshot
            .human_interactions
            .iter()
            .find(|interaction| {
                interaction.turn_id == turn_id && interaction.request.request_id == request_id
            })
            .cloned()
    }

    fn human_interaction_mut_or_err(
        &mut self,
        turn_id: TurnId,
        request_id: HumanInteractionRequestId,
    ) -> Result<&mut HumanInteractionSnapshot, AgentRuntimeError> {
        self.snapshot
            .human_interactions
            .iter_mut()
            .find(|interaction| {
                interaction.turn_id == turn_id && interaction.request.request_id == request_id
            })
            .ok_or_else(|| AgentRuntimeError::ProjectionFailed {
                message: format!("human interaction not found: {request_id}"),
            })
    }

    fn cancel_pending_approval_snapshots(&mut self, turn_id: TurnId) {
        let resolved_at = Utc::now();
        for approval in self.snapshot.approvals.iter_mut().filter(|approval| {
            approval.turn_id == turn_id && approval.status == ApprovalStatus::Pending
        }) {
            approval.status = ApprovalStatus::Canceled;
            approval.decision = Some(ApprovalDecision::Cancel);
            approval.resolved_at = Some(resolved_at);
        }
    }

    fn reasoning_snapshot(
        &self,
        turn_id: TurnId,
        message_id: Option<MessageId>,
    ) -> Option<ReasoningSnapshot> {
        self.snapshot
            .reasoning
            .iter()
            .find(|reasoning| reasoning.turn_id == turn_id && reasoning.message_id == message_id)
            .cloned()
    }

    fn plan_snapshot(&self, turn_id: TurnId) -> Option<PlanSnapshot> {
        self.snapshot
            .plans
            .iter()
            .find(|plan| plan.turn_id == turn_id)
            .cloned()
    }

    fn diff_snapshot(&self, turn_id: TurnId) -> Option<DiffSnapshot> {
        self.snapshot
            .diffs
            .iter()
            .find(|diff| diff.turn_id == turn_id)
            .cloned()
    }
}

#[derive(Debug, Clone)]
pub struct ChatProjector {
    default_thread_id: ThreadId,
    threads: HashMap<ThreadId, ThreadState>,
}

impl ChatProjector {
    #[must_use]
    pub fn new(config: ChatRuntimeConfig) -> Self {
        let thread_id = config.default_thread_id.unwrap_or_default();
        Self::with_default_thread(thread_id, config)
    }

    #[must_use]
    pub fn with_default_thread(thread_id: ThreadId, config: ChatRuntimeConfig) -> Self {
        let replay_capacity = config.replay_capacity;
        let mut threads = HashMap::new();
        threads.insert(thread_id, ThreadState::new(thread_id, replay_capacity));
        Self {
            default_thread_id: thread_id,
            threads,
        }
    }

    #[must_use]
    pub fn default_thread_id(&self) -> ThreadId {
        self.default_thread_id
    }

    #[must_use]
    pub fn read_snapshot(&self) -> RuntimeSnapshot {
        let mut threads = self
            .threads
            .values()
            .map(ThreadState::read_snapshot)
            .collect::<Vec<_>>();
        threads.sort_by_key(|thread| thread.thread_id.to_string());
        RuntimeSnapshot {
            schema_version: AGENT_RUNTIME_EVENT_SCHEMA_VERSION,
            threads,
        }
    }

    pub fn read_thread(&self, thread_id: ThreadId) -> Result<ThreadSnapshot, AgentRuntimeError> {
        self.thread(thread_id).map(ThreadState::read_snapshot)
    }

    pub fn turn_snapshot(&self, turn_id: TurnId) -> Result<TurnSnapshot, AgentRuntimeError> {
        self.thread(turn_id.thread_id())?
            .turn_snapshot_or_err(turn_id)
    }

    pub fn bootstrap_thread(
        &mut self,
        messages: ModelMessages,
    ) -> Result<ThreadSnapshot, AgentRuntimeError> {
        Ok(self.default_thread_mut().bootstrap_thread(messages))
    }

    pub fn bootstrap_thread_by_id(
        &mut self,
        thread_id: ThreadId,
        messages: ModelMessages,
    ) -> Result<ThreadSnapshot, AgentRuntimeError> {
        self.thread_mut(thread_id)
            .map(|thread| thread.bootstrap_thread(messages))
    }

    pub fn import_thread(
        &mut self,
        thread: ThreadSnapshot,
    ) -> Result<ThreadSnapshot, AgentRuntimeError> {
        let thread_id = thread.thread_id;
        let replay_capacity = self
            .threads
            .get(&thread_id)
            .or_else(|| self.threads.get(&self.default_thread_id))
            .map_or(ChatRuntimeConfig::default().replay_capacity, |thread| {
                thread.replay_capacity
            });
        self.threads.insert(
            thread_id,
            ThreadState::from_snapshot(thread, replay_capacity),
        );
        self.read_thread(thread_id)
    }

    pub fn events_after(
        &self,
        cursor: RuntimeCursor,
    ) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
        self.thread(cursor.thread_id)?.events_after(cursor)
    }

    #[must_use]
    pub fn queue_turn(&mut self, input: ModelMessages) -> TurnProjection {
        self.default_thread_mut().queue_turn(input)
    }

    pub fn start_turn(&mut self, turn_id: TurnId) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?.start_turn(turn_id)
    }

    pub fn complete_turn(
        &mut self,
        turn_id: TurnId,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?.complete_turn(turn_id)
    }

    pub fn fail_turn(
        &mut self,
        turn_id: TurnId,
        error: impl Into<String>,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .fail_turn(turn_id, error)
    }

    pub fn cancel_turn(&mut self, turn_id: TurnId) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?.cancel_turn(turn_id)
    }

    pub fn start_message(
        &mut self,
        turn_id: TurnId,
        payload: ModelMessage,
    ) -> Result<MessageProjection, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .start_message(turn_id, payload)
    }

    pub fn update_message(
        &mut self,
        message_id: MessageId,
        payload: ModelMessage,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(message_id.thread_id())?
            .update_message(message_id, payload)
    }

    pub fn complete_message(
        &mut self,
        message_id: MessageId,
        payload: ModelMessage,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(message_id.thread_id())?
            .complete_message(message_id, payload)
    }

    pub fn start_tool(
        &mut self,
        turn_id: TurnId,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        args: serde_json::Value,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .start_tool(turn_id, tool_call_id, tool_name, args)
    }

    pub fn update_tool(
        &mut self,
        turn_id: TurnId,
        tool_call_id: &str,
        partial_result: ToolUpdatePayload,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .update_tool(turn_id, tool_call_id, partial_result)
    }

    pub fn complete_tool(
        &mut self,
        turn_id: TurnId,
        tool_call_id: &str,
        final_result: AgentToolResult,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .complete_tool(turn_id, tool_call_id, final_result)
    }

    pub fn require_approval(
        &mut self,
        turn_id: TurnId,
        request: ApprovalRequest,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .require_approval(turn_id, request)
    }

    pub fn resolve_approval(
        &mut self,
        turn_id: TurnId,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .resolve_approval(turn_id, request_id, decision)
    }

    pub fn cancel_approval(
        &mut self,
        turn_id: TurnId,
        request_id: &str,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .cancel_approval(turn_id, request_id)
    }

    pub fn cancel_pending_approvals(
        &mut self,
        turn_id: TurnId,
    ) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .cancel_pending_approvals(turn_id)
    }

    pub fn request_human_interaction(
        &mut self,
        turn_id: TurnId,
        request: HumanInteractionRequest,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .request_human_interaction(turn_id, request)
    }

    pub fn resolve_human_interaction(
        &mut self,
        turn_id: TurnId,
        response: HumanInteractionResponse,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .resolve_human_interaction(turn_id, response)
    }

    pub fn cancel_human_interaction(
        &mut self,
        turn_id: TurnId,
        request_id: HumanInteractionRequestId,
        reason: Option<String>,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .cancel_human_interaction(turn_id, request_id, reason)
    }

    pub fn cancel_pending_human_interactions(
        &mut self,
        turn_id: TurnId,
    ) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .cancel_pending_human_interactions(turn_id)
    }

    pub fn update_reasoning(
        &mut self,
        turn_id: TurnId,
        message_id: Option<MessageId>,
        delta: impl Into<String>,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .update_reasoning(turn_id, message_id, delta)
    }

    pub fn update_plan(
        &mut self,
        turn_id: TurnId,
        plan: impl Into<String>,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .update_plan(turn_id, plan)
    }

    pub fn update_diff(
        &mut self,
        turn_id: TurnId,
        diff: impl Into<String>,
    ) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.thread_mut(turn_id.thread_id())?
            .update_diff(turn_id, diff)
    }

    fn thread(&self, thread_id: ThreadId) -> Result<&ThreadState, AgentRuntimeError> {
        self.threads
            .get(&thread_id)
            .ok_or(AgentRuntimeError::ThreadNotFound { thread_id })
    }

    fn thread_mut(&mut self, thread_id: ThreadId) -> Result<&mut ThreadState, AgentRuntimeError> {
        self.threads
            .get_mut(&thread_id)
            .ok_or(AgentRuntimeError::ThreadNotFound { thread_id })
    }

    fn default_thread_mut(&mut self) -> &mut ThreadState {
        self.threads
            .get_mut(&self.default_thread_id)
            .expect("default thread exists")
    }
}

impl Default for ChatProjector {
    fn default() -> Self {
        Self::new(ChatRuntimeConfig::default())
    }
}

fn is_terminal(status: TurnStatus) -> bool {
    matches!(
        status,
        TurnStatus::Completed | TurnStatus::Failed | TurnStatus::Canceled
    )
}
