//! Semantic snapshots, replay, and concurrent commands share one commit boundary.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{timeout, Duration};

use super::chat::{
    AgentRuntimeError, AgentRuntimeEvent, AgentRuntimeEventPayload, AgentRuntimeEventStore,
    ChatProjector, ChatRuntimeConfig, EnqueueTurnRequest, InMemoryAgentRuntimeEventStore,
    RuntimeCursor, ThreadId, TurnStatus,
};
use super::support::*;
use super::*;
use crate::session::{LocalSessionResources, LogicalPath};

const WAIT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy)]
enum GateTarget {
    Any,
    TurnStarted,
    TurnCanceled,
    Invalidate,
}

struct GatedCommitStore {
    inner: InMemoryAgentRuntimeEventStore,
    target: GateTarget,
    gates: Mutex<VecDeque<oneshot::Receiver<bool>>>,
    entered: mpsc::UnboundedSender<()>,
}

struct CommitControl {
    entered: mpsc::UnboundedReceiver<()>,
    gates: VecDeque<oneshot::Sender<bool>>,
}

impl CommitControl {
    async fn wait_for_append(&mut self) {
        timeout(WAIT, self.entered.recv())
            .await
            .expect("append should reach the gate")
            .expect("append gate should stay open");
    }

    fn release(&mut self, succeed: bool) {
        self.gates.pop_front().unwrap().send(succeed).unwrap();
    }
}

impl GatedCommitStore {
    fn new(target: GateTarget, count: usize) -> (Arc<Self>, CommitControl) {
        let (entered_tx, entered_rx) = mpsc::unbounded_channel();
        let (senders, receivers) = (0..count).map(|_| oneshot::channel()).unzip();
        (
            Arc::new(Self {
                inner: InMemoryAgentRuntimeEventStore::new(),
                target,
                gates: Mutex::new(receivers),
                entered: entered_tx,
            }),
            CommitControl {
                entered: entered_rx,
                gates: senders,
            },
        )
    }
}

#[async_trait]
impl AgentRuntimeEventStore for GatedCommitStore {
    async fn append(&self, event: AgentRuntimeEvent) -> Result<RuntimeCursor, AgentRuntimeError> {
        Ok(self.append_batch(vec![event]).await?.remove(0))
    }

    async fn append_batch(
        &self,
        events: Vec<AgentRuntimeEvent>,
    ) -> Result<Vec<RuntimeCursor>, AgentRuntimeError> {
        let matches = match self.target {
            GateTarget::Any => true,
            GateTarget::Invalidate => false,
            GateTarget::TurnStarted => events
                .iter()
                .any(|event| matches!(event.payload, AgentRuntimeEventPayload::TurnStarted { .. })),
            GateTarget::TurnCanceled => events.iter().any(|event| {
                matches!(event.payload, AgentRuntimeEventPayload::TurnCanceled { .. })
            }),
        };
        let gate = if matches {
            self.gates.lock().unwrap().pop_front()
        } else {
            None
        };
        if let Some(gate) = gate {
            self.entered.send(()).unwrap();
            if !gate.await.expect("test should release append") {
                return Err(AgentRuntimeError::ProjectionFailed {
                    message: "injected one-shot commit failure".into(),
                });
            }
        }
        self.inner.append_batch(events).await
    }

    async fn events_after(
        &self,
        cursor: RuntimeCursor,
    ) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
        self.inner.events_after(cursor).await
    }

    async fn invalidate_thread(
        &self,
        thread_id: ThreadId,
        latest_seq: u64,
    ) -> Result<(), AgentRuntimeError> {
        if matches!(self.target, GateTarget::Invalidate) {
            let gate = self.gates.lock().unwrap().pop_front();
            if let Some(gate) = gate {
                self.entered.send(()).unwrap();
                if !gate.await.expect("test should release invalidation") {
                    return Err(AgentRuntimeError::ProjectionFailed {
                        message: "injected invalidation failure".into(),
                    });
                }
            }
        }
        self.inner.invalidate_thread(thread_id, latest_seq).await
    }
}

fn commit_runtime(store: Arc<GatedCommitStore>) -> AgentRuntime {
    let mut config = test_agent_config();
    config.candidates = vec!["stub:commit".parse().unwrap()];
    config.chat.event_store = Some(store);
    AgentRuntime::new(
        registry_with_streaming_provider("stub", 8, 3),
        test_config(),
        config,
    )
}

fn turn(text: &str) -> EnqueueTurnRequest {
    EnqueueTurnRequest {
        messages: vec![ModelMessage::user(text)],
        generation_settings: None,
        approval_policy: None,
        collaboration_mode: None,
    }
}

async fn assert_replay_matches_snapshot(agent: &AgentRuntime) {
    let thread_id = agent.default_thread_id();
    let events = agent
        .subscribe(Some(RuntimeCursor::new(thread_id, 0)))
        .await
        .replay()
        .unwrap();
    assert!(events.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    let replay = ChatProjector::from_events(
        ChatRuntimeConfig {
            default_thread_id: Some(thread_id),
            ..Default::default()
        },
        events,
    )
    .unwrap();
    assert_eq!(agent.read_snapshot().await, replay.read_snapshot());
}

async fn assert_next_turn_completes(agent: &AgentRuntime, text: &str) {
    let turn_id = agent
        .enqueue_turn(turn(text))
        .await
        .expect("later turn should enqueue");
    timeout(WAIT, agent.wait_for_idle())
        .await
        .expect("later turn should finish");
    let thread = agent.read_thread(agent.default_thread_id()).await.unwrap();
    assert_eq!(
        thread
            .turns
            .iter()
            .find(|turn| turn.turn_id == turn_id)
            .unwrap()
            .status,
        TurnStatus::Completed
    );
    assert_replay_matches_snapshot(agent).await;
}

#[tokio::test]
async fn semantic_commit_snapshot_stays_unchanged_until_append_succeeds() {
    let (store, mut control) = GatedCommitStore::new(GateTarget::Any, 1);
    let agent = commit_runtime(store);
    let initial = agent.read_snapshot().await;
    let worker = agent.clone();
    let pending = tokio::spawn(async move { worker.enqueue_turn(turn("queued input")).await });
    control.wait_for_append().await;

    let while_blocked = timeout(WAIT, agent.read_snapshot())
        .await
        .expect("committed snapshot should remain readable during append");
    let mut subscription =
        Box::pin(agent.subscribe(Some(RuntimeCursor::new(agent.default_thread_id(), 0))));
    let replay_before_commit = match futures::poll!(subscription.as_mut()) {
        std::task::Poll::Ready(subscription) => Some(subscription.replay().unwrap()),
        std::task::Poll::Pending => None,
    };
    drop(subscription);
    control.release(true);
    pending.await.unwrap().unwrap();
    timeout(WAIT, agent.wait_for_idle()).await.unwrap();

    assert_eq!(
        while_blocked, initial,
        "uncommitted projection escaped through snapshot"
    );
    if let Some(replay) = replay_before_commit {
        assert!(
            replay.is_empty(),
            "replay must not expose the blocked append"
        );
    }
    assert_replay_matches_snapshot(&agent).await;
}

#[tokio::test]
async fn semantic_commit_failed_enqueue_does_not_erase_next_queued_turn() {
    let (store, mut control) = GatedCommitStore::new(GateTarget::Any, 2);
    let agent = commit_runtime(store);
    let first_agent = agent.clone();
    let first = tokio::spawn(async move { first_agent.enqueue_turn(turn("failed input")).await });
    control.wait_for_append().await;

    let second_agent = agent.clone();
    let mut second =
        Box::pin(async move { second_agent.enqueue_turn(turn("surviving input")).await });
    assert!(futures::poll!(second.as_mut()).is_pending());
    let second = tokio::spawn(second);
    control.release(false);
    let error = timeout(WAIT, first).await.unwrap().unwrap().unwrap_err();
    assert!(error
        .to_string()
        .contains("injected one-shot commit failure"));
    control.wait_for_append().await;
    control.release(true);
    let second_id = timeout(WAIT, second).await.unwrap().unwrap().unwrap();
    timeout(WAIT, agent.wait_for_idle()).await.unwrap();

    let snapshot = agent.read_thread(agent.default_thread_id()).await.unwrap();
    let surviving = snapshot
        .turns
        .iter()
        .find(|turn| turn.turn_id == second_id)
        .expect("successfully committed queued turn must survive the other enqueue's failure");
    assert_eq!(surviving.status, TurnStatus::Completed);
    assert!(snapshot
        .messages
        .iter()
        .all(|message| message.payload.text() != "failed input"));
    assert_replay_matches_snapshot(&agent).await;
    assert_next_turn_completes(&agent, "after recovery").await;
}

#[tokio::test]
async fn semantic_commit_failed_enqueue_does_not_erase_interleaved_artifact() {
    let (store, mut control) = GatedCommitStore::new(GateTarget::Any, 2);
    let mut agent = commit_runtime(store);
    let directory = tempfile::tempdir().unwrap();
    agent.session_resources = Some(Arc::new(
        LocalSessionResources::new(directory.path()).unwrap(),
    ));
    let first_agent = agent.clone();
    let first = tokio::spawn(async move { first_agent.enqueue_turn(turn("failed input")).await });
    control.wait_for_append().await;

    let resource_agent = agent.clone();
    let mut artifact = Box::pin(async move {
        resource_agent
            .write_artifact(LogicalPath::parse("result.txt").unwrap(), b"artifact data")
            .await
    });
    assert!(futures::poll!(artifact.as_mut()).is_pending());
    let artifact = tokio::spawn(artifact);
    control.release(false);
    assert!(timeout(WAIT, first).await.unwrap().unwrap().is_err());
    control.wait_for_append().await;
    control.release(true);
    let artifact = timeout(WAIT, artifact).await.unwrap().unwrap().unwrap();

    let snapshot = agent.read_thread(agent.default_thread_id()).await.unwrap();
    assert_eq!(snapshot.resources.artifacts, vec![artifact]);
    assert_eq!(
        std::fs::read(directory.path().join("artifacts/result.txt")).unwrap(),
        b"artifact data"
    );
    assert_replay_matches_snapshot(&agent).await;
    assert_next_turn_completes(&agent, "after artifact recovery").await;
}

#[tokio::test]
async fn semantic_commit_stream_failure_does_not_leave_completed_turn() {
    let (store, mut control) = GatedCommitStore::new(GateTarget::TurnStarted, 1);
    let agent = commit_runtime(store);
    let worker = agent.clone();
    let prompt = tokio::spawn(async move { worker.prompt("streamed input").await });
    control.wait_for_append().await;
    control.release(false);
    let error = timeout(WAIT, prompt).await.unwrap().unwrap().unwrap_err();
    assert!(error
        .to_string()
        .contains("injected one-shot commit failure"));

    let snapshot = agent.read_thread(agent.default_thread_id()).await.unwrap();
    assert_eq!(snapshot.turns.len(), 1);
    assert_eq!(
        snapshot.turns[0].status,
        TurnStatus::Failed,
        "a failed streamed commit must produce a committed failed turn"
    );
    assert_replay_matches_snapshot(&agent).await;
    assert_next_turn_completes(&agent, "after stream failure").await;
}

#[tokio::test]
async fn semantic_commit_cancel_only_aborts_after_successful_append() {
    let (store, mut control) = GatedCommitStore::new(GateTarget::TurnCanceled, 2);
    let provider_gate = Arc::new(tokio::sync::Notify::new());
    let mut config = test_agent_config();
    config.candidates = vec!["stub:cancel-commit".parse().unwrap()];
    config.chat.event_store = Some(store);
    let agent = AgentRuntime::new(
        registry_with_gated_streaming_provider("stub", provider_gate.clone()),
        test_config(),
        config,
    );
    let mut subscription = agent.subscribe(None).await;
    let turn_id = agent.enqueue_turn(turn("blocked provider")).await.unwrap();
    loop {
        let event = timeout(WAIT, subscription.recv()).await.unwrap().unwrap();
        if matches!(event.payload, AgentRuntimeEventPayload::TurnStarted { .. }) {
            break;
        }
    }
    let committed_running = agent.read_snapshot().await;
    let first_agent = agent.clone();
    let cancel = tokio::spawn(async move { first_agent.cancel_turn(turn_id).await });
    control.wait_for_append().await;
    let while_blocked = timeout(WAIT, agent.read_snapshot()).await.unwrap();
    let no_cancel_before_commit = timeout(Duration::from_millis(25), subscription.recv()).await;
    control.release(false);
    let error = timeout(WAIT, cancel).await.unwrap().unwrap().unwrap_err();

    assert!(error
        .to_string()
        .contains("injected one-shot commit failure"));
    assert_eq!(while_blocked, committed_running);
    assert!(
        no_cancel_before_commit.is_err(),
        "cancellation must not broadcast before commit"
    );
    assert_eq!(agent.read_snapshot().await, committed_running);
    assert_eq!(agent.state().await, AgentState::Running);
    assert_replay_matches_snapshot(&agent).await;

    let second_agent = agent.clone();
    let cancel = tokio::spawn(async move { second_agent.cancel_turn(turn_id).await });
    control.wait_for_append().await;
    control.release(true);
    let canceled = timeout(WAIT, cancel).await.unwrap().unwrap().unwrap();
    assert_eq!(canceled.status, TurnStatus::Canceled);
    timeout(WAIT, agent.wait_for_idle())
        .await
        .expect("committed cancellation should abort blocked provider");
    let event = timeout(WAIT, subscription.recv()).await.unwrap().unwrap();
    assert!(matches!(
        event.payload,
        AgentRuntimeEventPayload::TurnCanceled { .. }
    ));
    assert_replay_matches_snapshot(&agent).await;
    let events = agent
        .subscribe(Some(RuntimeCursor::new(agent.default_thread_id(), 0)))
        .await
        .replay()
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, AgentRuntimeEventPayload::TurnCanceled { .. }))
            .count(),
        1
    );

    provider_gate.notify_one();
    assert_next_turn_completes(&agent, "after committed cancellation").await;
}

#[tokio::test]
async fn semantic_commit_dropped_enqueue_caller_still_executes_and_drains_queue() {
    let (store, mut control) = GatedCommitStore::new(GateTarget::Any, 1);
    let agent = commit_runtime(store);
    let worker = agent.clone();
    let caller = tokio::spawn(async move { worker.enqueue_turn(turn("accepted input")).await });
    control.wait_for_append().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    control.release(true);

    timeout(WAIT, agent.wait_for_idle())
        .await
        .expect("accepted enqueue must execute and release its queued-turn count");
    let thread = agent.read_thread(agent.default_thread_id()).await.unwrap();
    assert_eq!(thread.turns.len(), 1);
    assert_eq!(thread.turns[0].status, TurnStatus::Completed);
    assert!(agent
        .messages()
        .await
        .iter()
        .any(|message| message.text() == "accepted input"));
    assert_replay_matches_snapshot(&agent).await;
    assert_next_turn_completes(&agent, "after dropped enqueue caller").await;
}

#[tokio::test]
async fn semantic_commit_dropped_cancel_caller_still_aborts_provider() {
    let (store, mut control) = GatedCommitStore::new(GateTarget::TurnCanceled, 1);
    let provider_gate = Arc::new(tokio::sync::Notify::new());
    let mut config = test_agent_config();
    config.candidates = vec!["stub:cancel-commit".parse().unwrap()];
    config.chat.event_store = Some(store);
    let agent = AgentRuntime::new(
        registry_with_gated_streaming_provider("stub", provider_gate.clone()),
        test_config(),
        config,
    );
    let mut events = agent.subscribe(None).await;
    let turn_id = agent.enqueue_turn(turn("blocked provider")).await.unwrap();
    loop {
        let event = timeout(WAIT, events.recv()).await.unwrap().unwrap();
        if matches!(event.payload, AgentRuntimeEventPayload::TurnStarted { .. }) {
            break;
        }
    }
    let worker = agent.clone();
    let caller = tokio::spawn(async move { worker.cancel_turn(turn_id).await });
    control.wait_for_append().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    control.release(true);

    timeout(WAIT, agent.wait_for_idle())
        .await
        .expect("accepted cancellation must abort provider after its caller drops");
    let thread = agent.read_thread(agent.default_thread_id()).await.unwrap();
    assert_eq!(thread.turns[0].status, TurnStatus::Canceled);
    assert_replay_matches_snapshot(&agent).await;
    provider_gate.notify_one();
    assert_next_turn_completes(&agent, "after dropped cancel caller").await;
}

#[derive(Clone, Copy, Debug)]
enum HistoryMutation {
    Replace,
    Import,
    Reset,
}

#[tokio::test]
async fn semantic_commit_dropped_history_caller_finishes_provider_history_update() {
    for operation in [
        HistoryMutation::Replace,
        HistoryMutation::Import,
        HistoryMutation::Reset,
    ] {
        let (store, mut control) = GatedCommitStore::new(GateTarget::Invalidate, 1);
        let agent = commit_runtime(store);
        agent.prompt("old history").await.unwrap();
        let expected = match operation {
            HistoryMutation::Reset => Vec::new(),
            _ => vec![ModelMessage::user("replacement history")],
        };
        let mut snapshots = agent.watch_snapshot();
        snapshots.borrow_and_update();
        let worker = agent.clone();
        let messages = expected.clone();
        let caller = tokio::spawn(async move {
            match operation {
                HistoryMutation::Replace => worker.replace_messages(messages).await.unwrap(),
                HistoryMutation::Import => {
                    let mut projector = ChatProjector::new(ChatRuntimeConfig {
                        default_thread_id: Some(worker.default_thread_id()),
                        ..Default::default()
                    });
                    let thread = projector.bootstrap_thread(messages.clone()).unwrap();
                    worker
                        .import_thread(super::chat::ImportedThread {
                            thread,
                            model_messages: messages,
                        })
                        .await
                        .unwrap();
                }
                HistoryMutation::Reset => worker.reset().await,
            }
        });
        control.wait_for_append().await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        control.release(true);

        timeout(WAIT, snapshots.changed()).await
            .unwrap_or_else(|_| panic!("{operation:?} must publish its final provider-history snapshot after caller cancellation"))
            .unwrap();
        assert_eq!(agent.messages().await, expected, "{operation:?}");
        assert_eq!(agent.state().await, AgentState::Idle, "{operation:?}");
        let thread = agent.read_thread(agent.default_thread_id()).await.unwrap();
        assert_eq!(
            thread
                .messages
                .iter()
                .map(|message| message.payload.clone())
                .collect::<Vec<_>>(),
            expected,
            "{operation:?}"
        );
        agent.prompt("after dropped history caller").await.unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn semantic_commit_first_poll_admits_direct_run_before_owned_task_executes() {
    // A current-thread runtime cannot schedule the owned task during these
    // synchronous polls, so admission cannot depend on task scheduling.
    for operation in [
        "prompt",
        "prompt_message",
        "continue_run",
        "continue_without_input",
    ] {
        let (store, _) = GatedCommitStore::new(GateTarget::Any, 0);
        let agent = commit_runtime(store);
        agent
            .replace_messages(vec![ModelMessage::user("seed history")])
            .await
            .unwrap();
        let mut caller = Box::pin(async {
            match operation {
                "prompt" => agent.prompt("admitted prompt").await,
                "prompt_message" => {
                    agent
                        .prompt_message(ModelMessage::user("admitted message"))
                        .await
                }
                "continue_run" => agent.continue_run("admitted continuation").await,
                "continue_without_input" => agent.continue_without_input().await,
                _ => unreachable!(),
            }
        });
        assert!(futures::poll!(caller.as_mut()).is_pending(), "{operation}");
        assert_eq!(
            agent.state().await,
            AgentState::Running,
            "{operation} must reserve execution on first poll"
        );
        let mut idle = Box::pin(agent.wait_for_idle());
        assert!(
            futures::poll!(idle.as_mut()).is_pending(),
            "{operation} must be visible to idle waiters immediately"
        );
        drop(idle);
        drop(caller);

        timeout(WAIT, agent.wait_for_idle())
            .await
            .expect("admitted direct run must survive waiter drop");
        let thread = agent.read_thread(agent.default_thread_id()).await.unwrap();
        assert_eq!(
            thread.turns.last().unwrap().status,
            TurnStatus::Completed,
            "{operation}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn semantic_commit_first_poll_admits_enqueue_before_owned_task_executes() {
    let (store, _) = GatedCommitStore::new(GateTarget::Any, 0);
    let agent = commit_runtime(store);
    let mut caller = Box::pin(agent.enqueue_turn(turn("admitted queued input")));
    assert!(futures::poll!(caller.as_mut()).is_pending());
    let mut idle = Box::pin(agent.wait_for_idle());
    assert!(
        futures::poll!(idle.as_mut()).is_pending(),
        "an accepted enqueue must count as outstanding work before the task runs"
    );
    drop(idle);
    drop(caller);

    timeout(WAIT, agent.wait_for_idle())
        .await
        .expect("accepted enqueue must drain after waiter drop");
    let thread = agent.read_thread(agent.default_thread_id()).await.unwrap();
    assert_eq!(thread.turns.len(), 1);
    assert_eq!(thread.turns[0].status, TurnStatus::Completed);
    assert_replay_matches_snapshot(&agent).await;
}

#[tokio::test]
async fn semantic_commit_abort_failure_does_not_abort_provider() {
    let (store, mut control) = GatedCommitStore::new(GateTarget::TurnCanceled, 1);
    let provider_gate = Arc::new(tokio::sync::Notify::new());
    let mut config = test_agent_config();
    config.candidates = vec!["stub:abort-commit".parse().unwrap()];
    config.chat.event_store = Some(store);
    let agent = AgentRuntime::new(
        registry_with_gated_streaming_provider("stub", provider_gate.clone()),
        test_config(),
        config,
    );
    let mut subscription = agent.subscribe(None).await;
    agent.enqueue_turn(turn("blocked provider")).await.unwrap();
    loop {
        let event = timeout(WAIT, subscription.recv()).await.unwrap().unwrap();
        if matches!(event.payload, AgentRuntimeEventPayload::TurnStarted { .. }) {
            break;
        }
    }
    let committed_running = agent.read_snapshot().await;
    let worker = agent.clone();
    let abort = tokio::spawn(async move { worker.abort().await });
    control.wait_for_append().await;
    control.release(false);
    assert!(!timeout(WAIT, abort).await.unwrap().unwrap());
    assert_eq!(agent.state().await, AgentState::Running);
    assert_eq!(agent.read_snapshot().await, committed_running);
    assert!(timeout(Duration::from_millis(25), agent.wait_for_idle())
        .await
        .is_err());
    assert_replay_matches_snapshot(&agent).await;

    assert!(agent.abort().await);
    timeout(WAIT, agent.wait_for_idle()).await.unwrap();
    provider_gate.notify_one();
    assert_next_turn_completes(&agent, "after rejected abort").await;
}

#[tokio::test]
async fn semantic_commit_abort_targets_provider_before_turn_started_commits() {
    let (store, mut control) = GatedCommitStore::new(GateTarget::TurnStarted, 1);
    let provider_gate = Arc::new(tokio::sync::Notify::new());
    let mut config = test_agent_config();
    config.candidates = vec!["stub:abort-queued".parse().unwrap()];
    config.chat.event_store = Some(store);
    let agent = AgentRuntime::new(
        registry_with_gated_streaming_provider("stub", provider_gate.clone()),
        test_config(),
        config,
    );
    let turn_id = agent.enqueue_turn(turn("blocked provider")).await.unwrap();
    control.wait_for_append().await;
    assert_eq!(agent.chat_turn_status(turn_id).unwrap(), TurnStatus::Queued);
    let worker = agent.clone();
    let mut abort = tokio::spawn(async move { worker.abort().await });
    assert!(
        timeout(Duration::from_millis(25), &mut abort)
            .await
            .is_err(),
        "abort must wait for its semantic cancellation to commit"
    );
    assert_eq!(agent.state().await, AgentState::Running);
    control.release(true);
    assert!(timeout(WAIT, abort).await.unwrap().unwrap());
    timeout(WAIT, agent.wait_for_idle()).await.unwrap();
    assert_eq!(
        agent.chat_turn_status(turn_id).unwrap(),
        TurnStatus::Canceled
    );
    assert_replay_matches_snapshot(&agent).await;
    provider_gate.notify_one();
    assert_next_turn_completes(&agent, "after queued provider abort").await;
}
