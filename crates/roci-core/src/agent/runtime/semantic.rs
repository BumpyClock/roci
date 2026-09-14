//! Serial owner of semantic projection, durable append, and committed visibility.
//!
//! Synchronous runner callbacks enqueue inputs without waiting for storage. The
//! queue remains unbounded until those callbacks can support async backpressure.
//! Dropping a caller's acknowledgement does not cancel an accepted mutation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, mpsc, oneshot, Notify};

use super::chat::{
    AgentRuntimeError, AgentRuntimeEvent, AgentRuntimeEventPayload, AgentRuntimeEventStore,
    ChatProjector, CollaborationMode, RuntimeCursor, RuntimeSnapshot, RuntimeSubscription,
    ThreadId, ThreadSnapshot, TurnId, TurnSnapshot, TurnStatus,
};
use super::events::{project_agent_event, project_plan_update_and_mirror, ChatProjectionRunState};
use crate::agent_loop::{AgentEvent, RetryEvent};
use crate::session::LocalSessionResources;
use crate::types::ModelMessage;

/// First semantic failure for one runner, with a wakeup for blocked provider/tool work.
#[derive(Clone, Default)]
pub(super) struct SemanticRunErrors {
    inner: Arc<SemanticRunErrorsInner>,
}

#[derive(Default)]
struct SemanticRunErrorsInner {
    error: Mutex<Option<AgentRuntimeError>>,
    changed: Notify,
}

impl SemanticRunErrors {
    pub(super) fn take(&self) -> Option<AgentRuntimeError> {
        self.inner
            .error
            .lock()
            .expect("semantic error slot poisoned")
            .take()
    }

    fn current(&self) -> Option<AgentRuntimeError> {
        self.inner
            .error
            .lock()
            .expect("semantic error slot poisoned")
            .clone()
    }

    fn record(&self, error: AgentRuntimeError) {
        let mut current = self
            .inner
            .error
            .lock()
            .expect("semantic error slot poisoned");
        if current.is_none() {
            *current = Some(error);
            drop(current);
            self.inner.changed.notify_waiters();
        }
    }

    pub(super) async fn wait(&self) {
        loop {
            let notified = self.inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.current().is_some() {
                return;
            }
            notified.await;
        }
    }
}

#[derive(Clone)]
pub(super) struct SemanticRuntime {
    inner: Arc<SemanticRuntimeInner>,
}

struct SemanticRuntimeInner {
    tx: mpsc::UnboundedSender<Command>,
    rx: Mutex<Option<mpsc::UnboundedReceiver<Command>>>,
    committed: Arc<Mutex<Arc<ChatProjector>>>,
    store: Arc<dyn AgentRuntimeEventStore>,
    events: broadcast::Sender<AgentRuntimeEvent>,
}

#[derive(Clone)]
struct SemanticState {
    projector: Arc<ChatProjector>,
    runs: HashMap<TurnId, ChatProjectionRunState>,
}

enum Commit {
    Append(Vec<AgentRuntimeEvent>),
    Invalidate(ThreadId, u64),
    None,
}

enum Command {
    Mutation(Box<dyn Mutation>),
    Flush(oneshot::Sender<()>),
    Subscribe(Option<RuntimeCursor>, oneshot::Sender<RuntimeSubscription>),
}

trait Mutation: Send {
    fn apply(&mut self, state: &mut SemanticState) -> Result<Commit, AgentRuntimeError>;
    fn finish(self: Box<Self>, result: Result<(), AgentRuntimeError>);
}

struct Pending<T, F> {
    operation: Option<F>,
    value: Option<T>,
    ack: Option<oneshot::Sender<Result<T, AgentRuntimeError>>>,
    error_slot: Option<SemanticRunErrors>,
}

impl<T, F> Mutation for Pending<T, F>
where
    T: Send + 'static,
    F: FnOnce(&mut SemanticState) -> Result<(T, Commit), AgentRuntimeError> + Send + 'static,
{
    fn apply(&mut self, state: &mut SemanticState) -> Result<Commit, AgentRuntimeError> {
        if let Some(slot) = &self.error_slot {
            if let Some(error) = slot.current() {
                return Err(error);
            }
        }
        let (value, commit) = self.operation.take().expect("mutation applied once")(state)?;
        self.value = Some(value);
        Ok(commit)
    }

    fn finish(mut self: Box<Self>, result: Result<(), AgentRuntimeError>) {
        if let (Err(error), Some(slot)) = (&result, &self.error_slot) {
            slot.record(error.clone());
        }
        if let Some(ack) = self.ack.take() {
            let _ = ack.send(
                result.map(|()| self.value.take().expect("successful mutation has a result")),
            );
        }
    }
}

fn failed(message: impl Into<String>) -> AgentRuntimeError {
    AgentRuntimeError::ProjectionFailed {
        message: message.into(),
    }
}

impl SemanticRuntime {
    pub(super) fn new(
        projector: ChatProjector,
        store: Arc<dyn AgentRuntimeEventStore>,
        broadcast_capacity: usize,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(broadcast_capacity.max(1));
        Self {
            inner: Arc::new(SemanticRuntimeInner {
                tx,
                rx: Mutex::new(Some(rx)),
                committed: Arc::new(Mutex::new(Arc::new(projector))),
                store,
                events,
            }),
        }
    }

    fn start(&self) -> Result<(), AgentRuntimeError> {
        let mut receiver = self
            .inner
            .rx
            .lock()
            .map_err(|_| failed("semantic receiver poisoned"))?;
        if receiver.is_none() {
            return Ok(());
        }
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| failed("semantic mutation requires a Tokio runtime"))?;
        let rx = receiver.take().expect("receiver checked above");
        let committed = self.inner.committed.clone();
        let state = SemanticState {
            projector: committed
                .lock()
                .map_err(|_| failed("semantic snapshot poisoned"))?
                .clone(),
            runs: HashMap::new(),
        };
        runtime.spawn(drive(
            rx,
            state,
            committed,
            self.inner.store.clone(),
            self.inner.events.clone(),
        ));
        Ok(())
    }

    fn read(&self) -> Arc<ChatProjector> {
        self.inner
            .committed
            .lock()
            .expect("semantic snapshot poisoned")
            .clone()
    }

    pub(super) fn default_thread_id(&self) -> ThreadId {
        self.read().default_thread_id()
    }
    pub(super) fn read_snapshot(&self) -> RuntimeSnapshot {
        self.read().read_snapshot()
    }
    pub(super) fn read_thread(&self, id: ThreadId) -> Result<ThreadSnapshot, AgentRuntimeError> {
        self.read().read_thread(id)
    }
    pub(super) fn turn_snapshot(&self, id: TurnId) -> Result<TurnSnapshot, AgentRuntimeError> {
        self.read().turn_snapshot(id)
    }

    pub(super) async fn subscribe(&self, cursor: Option<RuntimeCursor>) -> RuntimeSubscription {
        let (ack, response) = oneshot::channel();
        let result = match self.start() {
            Ok(()) => match self.inner.tx.send(Command::Subscribe(cursor, ack)) {
                Ok(()) => response
                    .await
                    .map_err(|_| failed("semantic owner dropped subscription acknowledgement")),
                Err(_) => Err(failed("semantic owner closed")),
            },
            Err(error) => Err(error),
        };
        result.unwrap_or_else(|error| {
            RuntimeSubscription::new(Err(error), self.inner.events.subscribe(), cursor)
        })
    }

    async fn mutate<T, F>(&self, operation: F) -> Result<T, AgentRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&mut SemanticState) -> Result<(T, Commit), AgentRuntimeError> + Send + 'static,
    {
        self.start()?;
        let (ack, response) = oneshot::channel();
        self.inner
            .tx
            .send(Command::Mutation(Box::new(Pending {
                operation: Some(operation),
                value: None,
                ack: Some(ack),
                error_slot: None,
            })))
            .map_err(|_| failed("semantic owner closed"))?;
        response
            .await
            .map_err(|_| failed("semantic owner dropped acknowledgement"))?
    }

    pub(super) async fn transact<T, F>(&self, operation: F) -> Result<T, AgentRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&mut ChatProjector) -> Result<(T, Vec<AgentRuntimeEvent>), AgentRuntimeError>
            + Send
            + 'static,
    {
        self.mutate(move |state| {
            let (value, events) = operation(Arc::make_mut(&mut state.projector))?;
            Ok((value, Commit::Append(events)))
        })
        .await
    }

    pub(super) async fn queue_turn(
        &self,
        messages: Vec<ModelMessage>,
    ) -> Result<TurnId, AgentRuntimeError> {
        self.transact(move |projector| {
            let projection = projector.queue_turn(messages);
            Ok((projection.turn_id, projection.events))
        })
        .await
    }

    pub(super) async fn cancel_turn(
        &self,
        turn_id: TurnId,
    ) -> Result<TurnSnapshot, AgentRuntimeError> {
        self.transact(move |projector| {
            let mut events = projector.cancel_pending_approvals(turn_id)?;
            events.extend(projector.cancel_pending_human_interactions(turn_id)?);
            events.push(projector.cancel_turn(turn_id)?);
            Ok((projector.turn_snapshot(turn_id)?, events))
        })
        .await
    }

    pub(super) async fn terminal_turn(
        &self,
        turn_id: TurnId,
        status: TurnStatus,
        error: Option<String>,
    ) -> Result<(), AgentRuntimeError> {
        self.mutate(move |state| {
            let projector = Arc::make_mut(&mut state.projector);
            let previous = projector.turn_snapshot(turn_id)?.status;
            if previous == status {
                state.runs.remove(&turn_id);
                return Ok(((), Commit::None));
            }
            let mut events = Vec::new();
            let terminal = match status {
                TurnStatus::Completed => projector.complete_turn(turn_id)?,
                TurnStatus::Failed => {
                    projector.fail_turn(turn_id, error.unwrap_or_else(|| "run failed".into()))?
                }
                TurnStatus::Canceled => {
                    events.extend(projector.cancel_pending_approvals(turn_id)?);
                    events.extend(projector.cancel_pending_human_interactions(turn_id)?);
                    projector.cancel_turn(turn_id)?
                }
                _ => return Err(failed("terminal mutation requires terminal status")),
            };
            events.push(terminal);
            state.runs.remove(&turn_id);
            Ok(((), Commit::Append(events)))
        })
        .await
    }

    pub(super) async fn resource(
        &self,
        payload: AgentRuntimeEventPayload,
    ) -> Result<(), AgentRuntimeError> {
        self.transact(move |projector| {
            let thread_id = projector.default_thread_id();
            let event = match projector.read_thread(thread_id)?.active_turn_id {
                Some(turn_id) => projector.record_session_resource(turn_id, payload)?,
                None => projector.record_session_resource_for_thread(thread_id, payload)?,
            };
            Ok(((), vec![event]))
        })
        .await
    }

    pub(super) async fn plan(
        &self,
        turn_id: TurnId,
        plan: String,
        resources: Option<Arc<LocalSessionResources>>,
    ) -> Result<(), AgentRuntimeError> {
        self.transact(move |projector| {
            let events =
                project_plan_update_and_mirror(projector, turn_id, plan, resources.as_deref())?;
            Ok(((), events))
        })
        .await
    }

    pub(super) async fn import_thread(
        &self,
        snapshot: ThreadSnapshot,
        invalidate: bool,
    ) -> Result<ThreadSnapshot, AgentRuntimeError> {
        self.mutate(move |state| {
            let snapshot = Arc::make_mut(&mut state.projector).import_thread(snapshot)?;
            state
                .runs
                .retain(|turn, _| turn.thread_id() != snapshot.thread_id);
            let commit = if invalidate {
                Commit::Invalidate(snapshot.thread_id, snapshot.last_seq)
            } else {
                Commit::None
            };
            Ok((snapshot, commit))
        })
        .await
    }

    pub(super) async fn bootstrap(
        &self,
        messages: Vec<ModelMessage>,
    ) -> Result<ThreadSnapshot, AgentRuntimeError> {
        self.mutate(move |state| {
            let snapshot = Arc::make_mut(&mut state.projector).bootstrap_thread(messages)?;
            state
                .runs
                .retain(|turn, _| turn.thread_id() != snapshot.thread_id);
            Ok((
                snapshot.clone(),
                Commit::Invalidate(snapshot.thread_id, snapshot.last_seq),
            ))
        })
        .await
    }

    fn enqueue<F>(&self, operation: F, error_slot: SemanticRunErrors)
    where
        F: FnOnce(&mut SemanticState) -> Result<((), Commit), AgentRuntimeError> + Send + 'static,
    {
        if let Err(error) = self.start() {
            error_slot.record(error);
            return;
        }
        if self
            .inner
            .tx
            .send(Command::Mutation(Box::new(Pending {
                operation: Some(operation),
                value: None,
                ack: None,
                error_slot: Some(error_slot.clone()),
            })))
            .is_err()
        {
            error_slot.record(failed("semantic owner closed"));
        }
    }

    pub(super) fn event(
        &self,
        turn_id: TurnId,
        event: AgentEvent,
        initial_count: usize,
        mode: CollaborationMode,
        resources: Option<Arc<LocalSessionResources>>,
        error_slot: SemanticRunErrors,
    ) {
        self.enqueue(
            move |state| {
                let run = state
                    .runs
                    .entry(turn_id)
                    .or_insert_with(|| ChatProjectionRunState::new(initial_count, mode));
                let events = project_agent_event(
                    Arc::make_mut(&mut state.projector),
                    run,
                    turn_id,
                    &event,
                    resources.as_deref(),
                )?;
                Ok(((), Commit::Append(events)))
            },
            error_slot,
        );
    }

    pub(super) fn retry(&self, turn_id: TurnId, event: RetryEvent, error_slot: SemanticRunErrors) {
        self.enqueue(
            move |state| {
                let event = Arc::make_mut(&mut state.projector).record_retry(turn_id, event)?;
                Ok(((), Commit::Append(vec![event])))
            },
            error_slot,
        );
    }

    pub(super) async fn flush(&self) -> Result<(), AgentRuntimeError> {
        self.start()?;
        let (ack, response) = oneshot::channel();
        self.inner
            .tx
            .send(Command::Flush(ack))
            .map_err(|_| failed("semantic owner closed"))?;
        response
            .await
            .map_err(|_| failed("semantic owner dropped flush acknowledgement"))
    }
}

async fn drive(
    mut rx: mpsc::UnboundedReceiver<Command>,
    mut state: SemanticState,
    committed: Arc<Mutex<Arc<ChatProjector>>>,
    store: Arc<dyn AgentRuntimeEventStore>,
    events_tx: broadcast::Sender<AgentRuntimeEvent>,
) {
    while let Some(command) = rx.recv().await {
        match command {
            Command::Flush(ack) => {
                let _ = ack.send(());
            }
            Command::Subscribe(cursor, ack) => {
                let live = events_tx.subscribe();
                let replay = match cursor {
                    Some(cursor) => store.events_after(cursor).await,
                    None => Ok(Vec::new()),
                };
                let _ = ack.send(RuntimeSubscription::new(replay, live, cursor));
            }
            Command::Mutation(mut mutation) => {
                let mut staged = state.clone();
                let result = match mutation.apply(&mut staged) {
                    Ok(commit) => {
                        let (result, events) = match commit {
                            Commit::Append(events) if events.is_empty() => (Ok(()), events),
                            Commit::Append(events) => {
                                (store.append_batch(events.clone()).await.map(|_| ()), events)
                            }
                            Commit::Invalidate(thread_id, seq) => {
                                (store.invalidate_thread(thread_id, seq).await, Vec::new())
                            }
                            Commit::None => (Ok(()), Vec::new()),
                        };
                        match result {
                            Ok(()) => match committed.lock() {
                                Ok(mut visible) => {
                                    *visible = staged.projector.clone();
                                    state = staged;
                                    for event in events {
                                        let _ = events_tx.send(event);
                                    }
                                    Ok(())
                                }
                                Err(_) => Err(failed("semantic snapshot poisoned")),
                            },
                            Err(error) => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                };
                mutation.finish(result);
            }
        }
    }
}
