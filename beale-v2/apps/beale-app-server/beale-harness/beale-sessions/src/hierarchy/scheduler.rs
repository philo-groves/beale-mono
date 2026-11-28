//! Detached, event-driven Swarm scheduling.
//!
//! One scheduler task per running scan owns the scan's session lock. It
//! launches every eligible agent on its own task, so the leader and its
//! subagents run concurrently within the durable capacity and ownership
//! gates. Requests attach to the scan's hub: they receive its live events and
//! its final outcome, and dropping a request does not stop the scan. A second
//! submission to a running scan queues an operator message for the leader and
//! attaches to the same stream.
//!
//! Durable state remains the source of truth. Wake signals only shorten the
//! wait before the next scheduling pass; a fallback tick covers any missed
//! wake.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Notify, OwnedMutexGuard, broadcast};
use tokio::task::JoinSet;

/// Timing for scheduling passes and live status.
#[derive(Clone, Copy, Debug)]
struct Tuning {
    /// Fallback pass interval; also reruns recovery and reconciliation.
    tick: Duration,
    /// Minimum time between passes, coalescing bursts of wakes.
    min_pass: Duration,
    /// Delay after a change before publishing live status.
    publish_debounce: Duration,
    /// Live status interval without changes.
    publish_fallback: Duration,
}

const TUNING: Tuning = if cfg!(test) {
    Tuning {
        tick: Duration::from_millis(200),
        min_pass: Duration::from_millis(5),
        publish_debounce: Duration::from_millis(10),
        publish_fallback: Duration::from_millis(250),
    }
} else {
    Tuning {
        tick: Duration::from_secs(2),
        min_pass: Duration::from_millis(50),
        publish_debounce: Duration::from_millis(100),
        publish_fallback: Duration::from_secs(1),
    }
};

/// Live events kept for a slow subscriber before it skips ahead. Status
/// snapshots are complete, so a subscriber that lags recovers at the next one.
const EVENT_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HubPhase {
    /// The first request is creating or reopening the scan.
    Starting,
    /// Agents are scheduled; new submissions attach.
    Running,
    /// A stop was requested; in-flight agents finish and nothing new starts.
    Stopping,
    /// Scheduling ended; the scan is closing out.
    Closing,
    /// The outcome is published and the hub has left the registry.
    Closed,
}

type Outcome = Option<Result<SessionDetail, TurnError>>;

pub(crate) struct ScanHub {
    scan: String,
    wake: Notify,
    publish: Notify,
    /// Counts wake signals so an idle scheduler can tell whether anything
    /// changed after its last pass began.
    wakes: AtomicU64,
    events: broadcast::Sender<SessionStreamEvent>,
    stop: watch::Sender<bool>,
    phase: watch::Sender<HubPhase>,
    done: watch::Sender<Outcome>,
}

impl ScanHub {
    fn new(scan: String) -> Arc<Self> {
        Arc::new(Self {
            scan,
            wake: Notify::new(),
            publish: Notify::new(),
            wakes: AtomicU64::new(0),
            events: broadcast::channel(EVENT_CAPACITY).0,
            stop: watch::channel(false).0,
            phase: watch::channel(HubPhase::Starting).0,
            done: watch::channel(None).0,
        })
    }

    fn signal(&self) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
        self.wake.notify_one();
        self.publish.notify_one();
    }

    fn phase(&self) -> HubPhase {
        *self.phase.borrow()
    }

    fn attach(self: &Arc<Self>) -> Attachment {
        Attachment {
            hub: self.clone(),
            events: self.events.subscribe(),
            done: self.done.subscribe(),
        }
    }
}

/// Running scans on this host, and the signal for native capacity changes.
#[derive(Default)]
pub(crate) struct Registry {
    scans: std::sync::Mutex<HashMap<String, Arc<ScanHub>>>,
    /// Notified when native allocations are bound, released, or reconciled,
    /// so queued reservations retry admission promptly.
    pub(crate) allocations_changed: Notify,
}

impl Registry {
    fn scans(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<ScanHub>>> {
        // A panic while holding this lock cannot leave the map inconsistent:
        // every critical section is a single insert, lookup, or removal.
        self.scans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Signals that durable state for `scan` may have changed. Does nothing when
/// the scan has no scheduler on this host.
pub(crate) fn wake(state: &SessionState, scan: &str) {
    if let Some(hub) = state.swarm_scans.scans().get(scan) {
        hub.signal();
    }
}

/// Signals every running scan, after host-wide maintenance.
pub(crate) fn wake_all(state: &SessionState) {
    for hub in state.swarm_scans.scans().values() {
        hub.signal();
    }
    state.swarm_scans.allocations_changed.notify_waiters();
}

/// A request's view of a running scan.
pub(crate) struct Attachment {
    hub: Arc<ScanHub>,
    events: broadcast::Receiver<SessionStreamEvent>,
    done: watch::Receiver<Outcome>,
}

impl Attachment {
    /// Forwards live events to `progress` until the scan's scheduler finishes,
    /// and returns its outcome. A stop on `stop` stops the scan. Dropping this
    /// future, or a closed `progress`, only detaches.
    pub(crate) async fn follow(
        self,
        progress: Option<mpsc::Sender<SessionStreamEvent>>,
        mut stop: Option<watch::Receiver<bool>>,
    ) -> Result<SessionDetail, TurnError> {
        let Self {
            hub,
            events,
            mut done,
        } = self;
        // Without a progress channel an unread receiver would only lag.
        let mut events = progress.as_ref().map(|_| events);
        let mut stop_sent = false;
        loop {
            let finished = done.borrow_and_update().clone();
            if let Some(outcome) = finished {
                if let (Some(events), Some(tx)) = (events.as_mut(), &progress) {
                    // Deliver events already published before the outcome,
                    // without waiting on a client that stopped reading.
                    while let Ok(event) = events.try_recv() {
                        if tx.try_send(event).is_err() {
                            break;
                        }
                    }
                }
                return outcome;
            }
            if !stop_sent && engine::stop_requested(&stop) {
                hub.stop.send_replace(true);
                stop_sent = true;
            }
            tokio::select! {
                biased;
                changed = done.changed() => {
                    if changed.is_err() {
                        return Err(TurnError::Internal);
                    }
                }
                changed = async {
                    match stop.as_mut() {
                        Some(receiver) => receiver.changed().await,
                        None => std::future::pending().await,
                    }
                }, if !stop_sent => {
                    if changed.is_err() {
                        stop = None;
                    }
                }
                event = async {
                    match events.as_mut() {
                        Some(receiver) => receiver.recv().await,
                        None => std::future::pending().await,
                    }
                } => match event {
                    Ok(event) => {
                        if let Some(tx) = &progress {
                            // A client that stops reading delays only its own
                            // stream; the outcome still ends this attachment.
                            tokio::select! {
                                biased;
                                sent = tx.send(event) => {
                                    if sent.is_err() {
                                        events = None;
                                    }
                                }
                                _ = done.changed() => {}
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => events = None,
                },
            }
        }
    }
}

enum Entry {
    Wait(Arc<ScanHub>),
    Start(OwnedMutexGuard<()>, Arc<Mutex<()>>, Arc<ScanHub>),
}

/// Starts a scheduler for `request`, or attaches to the scan's running one.
pub(crate) async fn start_or_attach(
    state: &SessionState,
    request: SubmitSessionTurnRequest,
) -> Result<Attachment, TurnError> {
    let scan = validate_start(state, &request)?;
    loop {
        let lock = session_lock(state, &scan).await;
        let entry = {
            let mut scans = state.swarm_scans.scans();
            match scans.get(&scan) {
                Some(hub) => match hub.phase() {
                    HubPhase::Running => {
                        // Queue while holding the registry, so an idle
                        // scheduler deciding to close sees this wake.
                        queue_follow_up(state, &request, &scan)?;
                        let attachment = hub.attach();
                        hub.signal();
                        return Ok(attachment);
                    }
                    HubPhase::Stopping => return Err(TurnError::Conflict),
                    HubPhase::Starting | HubPhase::Closing | HubPhase::Closed => {
                        Entry::Wait(hub.clone())
                    }
                },
                None => {
                    // Ordinary turns and archive hold the same lock.
                    let guard = lock
                        .clone()
                        .try_lock_owned()
                        .map_err(|_| TurnError::Conflict)?;
                    let hub = ScanHub::new(scan.clone());
                    scans.insert(scan.clone(), hub.clone());
                    Entry::Start(guard, lock, hub)
                }
            }
        };
        match entry {
            Entry::Wait(hub) => {
                let mut phase = hub.phase.subscribe();
                let _ = phase
                    .wait_for(|phase| matches!(phase, HubPhase::Running | HubPhase::Closed))
                    .await;
            }
            Entry::Start(guard, lock, hub) => {
                let attachment = hub.attach();
                // Setup and scheduling run detached: a request dropped during
                // setup cannot leave the hub registered without a scheduler.
                let (ready, prepared) = tokio::sync::oneshot::channel();
                tokio::spawn({
                    let state = state.clone();
                    let scan = scan.clone();
                    async move {
                        if let Err(error) = prepare(&state, &request, &scan).await {
                            close(&state, &hub, guard, Err(error));
                            let _ = ready.send(Err(error));
                            return;
                        }
                        hub.phase.send_replace(HubPhase::Running);
                        let _ = ready.send(Ok(()));
                        schedule(state, hub, request, lock, guard).await;
                    }
                });
                return match prepared.await {
                    Ok(Ok(())) => Ok(attachment),
                    Ok(Err(error)) => Err(error),
                    Err(_) => Err(TurnError::Internal),
                };
            }
        }
    }
}

/// Publishes the outcome and removes the hub. The session lock is released
/// inside the registry, so a request that finds no hub can take the lock.
fn close(
    state: &SessionState,
    hub: &Arc<ScanHub>,
    guard: OwnedMutexGuard<()>,
    outcome: Result<SessionDetail, TurnError>,
) {
    let mut scans = state.swarm_scans.scans();
    if scans
        .get(&hub.scan)
        .is_some_and(|current| Arc::ptr_eq(current, hub))
    {
        scans.remove(&hub.scan);
    }
    drop(guard);
    hub.done.send_replace(Some(outcome));
    hub.phase.send_replace(HubPhase::Closed);
}

async fn schedule(
    state: SessionState,
    hub: Arc<ScanHub>,
    request: SubmitSessionTurnRequest,
    lock: Arc<Mutex<()>>,
    guard: OwnedMutexGuard<()>,
) {
    let publisher = tokio::spawn(publish(state.clone(), hub.clone(), request.clone()));
    // A panic while scheduling still publishes an outcome and removes the hub.
    let run = tokio::spawn({
        let state = state.clone();
        let hub = hub.clone();
        async move {
            match drive(&state, &hub, &request, &lock).await {
                Ok(stopped) => closeout(&state, &hub.scan, &request, stopped).await,
                Err(error) => Err(error),
            }
        }
    });
    let outcome = run.await.unwrap_or_else(|error| {
        beale_host_log::error!("Swarm scheduler failed: {error}");
        Err(TurnError::Internal)
    });
    publisher.abort();
    close(&state, &hub, guard, outcome);
}

/// What a launch task did.
enum Launched {
    /// The agent executed, or its launch changed durable state.
    Ran,
    /// Nothing was claimable for the agent.
    Idle,
}

/// Runs scheduling passes until no agent is running or eligible. Returns
/// whether the scan was stopped.
async fn drive(
    state: &SessionState,
    hub: &Arc<ScanHub>,
    request: &SubmitSessionTurnRequest,
    lock: &Arc<Mutex<()>>,
) -> Result<bool, TurnError> {
    let scan = hub.scan.clone();
    let mut stop = hub.stop.subscribe();
    let mut tasks = JoinSet::new();
    // Each launch task's agent and the wake count its pass started from.
    let mut launched = HashMap::<tokio::task::Id, (String, u64)>::new();
    // Each in-flight agent's stop signal: set when the scan stops or when the
    // agent's branch is stopped while it runs.
    let mut in_flight = HashMap::<String, watch::Sender<bool>>::new();
    // Agents whose launch found nothing to claim, with the wake count that
    // launch started from. An agent stays skipped only while no wake has
    // arrived since; a wake counted by a later pass must not hide it.
    let mut dormant = HashMap::<String, u64>::new();
    let workers = Arc::new(tokio::sync::OnceCell::<distributed::LocalWorkerHost>::new());
    let mut stopping = false;
    let mut failure = None;
    let mut maintenance = true;
    let mut last_pass: Option<tokio::time::Instant> = None;
    loop {
        if !stopping && *stop.borrow_and_update() {
            stopping = true;
            hub.phase.send_replace(HubPhase::Stopping);
            hub.publish.notify_one();
            for agent_stop in in_flight.values() {
                agent_stop.send_replace(true);
            }
        }
        let seen = hub.wakes.load(Ordering::SeqCst);
        if !stopping && failure.is_none() {
            if let Some(previous) = last_pass {
                let elapsed = previous.elapsed();
                if elapsed < TUNING.min_pass {
                    tokio::time::sleep(TUNING.min_pass - elapsed).await;
                }
            }
            last_pass = Some(tokio::time::Instant::now());
            match pass(state, &scan, maintenance).await {
                Ok(view) => {
                    maintenance = false;
                    for agent in &view.branch_stopped {
                        if let Some(agent_stop) = in_flight.get(agent) {
                            agent_stop.send_replace(true);
                        }
                    }
                    for candidate in view.candidates {
                        if in_flight.contains_key(&candidate.agent)
                            || dormant.get(&candidate.agent) == Some(&seen)
                        {
                            continue;
                        }
                        let agent = candidate.agent.clone();
                        let (agent_stop, agent_stopped) = watch::channel(false);
                        let handle = tasks.spawn(launch(
                            state.clone(),
                            scan.clone(),
                            request.workspace_id.clone(),
                            candidate,
                            lock.clone(),
                            agent_stopped,
                            hub.events.clone(),
                            workers.clone(),
                        ));
                        launched.insert(handle.id(), (agent.clone(), seen));
                        in_flight.insert(agent, agent_stop);
                    }
                }
                Err(error) => {
                    failure = Some(error);
                }
            }
        }
        if tasks.is_empty() {
            if failure.is_some() {
                break;
            }
            if stopping {
                // Stop the branch only after every execution finished, as a
                // running turn records its own stopped answer first.
                let saved = store(&state.db)?
                    .load(&scan, true)
                    .map_err(|_| TurnError::Internal)?;
                let leader = saved
                    .agents
                    .iter()
                    .find(|a| a.summary.role == Role::Leader)
                    .ok_or(TurnError::Internal)?;
                store(&state.db)?
                    .local_stop(&scan, &leader.summary.id)
                    .map_err(|_| TurnError::Internal)?;
                hub.phase.send_replace(HubPhase::Closing);
                break;
            }
            {
                let _scans = state.swarm_scans.scans();
                if hub.wakes.load(Ordering::SeqCst) == seen {
                    hub.phase.send_replace(HubPhase::Closing);
                    break;
                }
            }
            dormant.clear();
            continue;
        }
        tokio::select! {
            changed = stop.changed(), if !stopping => {
                if changed.is_err() {
                    stopping = true;
                }
            }
            joined = tasks.join_next_with_id() => {
                maintenance = true;
                state.swarm_scans.allocations_changed.notify_waiters();
                hub.publish.notify_one();
                let (id, result) = match joined {
                    Some(Ok((id, result))) => (id, result),
                    Some(Err(error)) => {
                        beale_host_log::error!("Swarm agent task failed: {error}");
                        (error.id(), Err(TurnError::Internal))
                    }
                    None => continue,
                };
                if let Some((agent, started)) = launched.remove(&id) {
                    in_flight.remove(&agent);
                    match result {
                        Ok(Launched::Ran) => {
                            dormant.clear();
                        }
                        Ok(Launched::Idle) => {
                            dormant.insert(agent, started);
                        }
                        Err(error) => {
                            failure.get_or_insert(error);
                        }
                    }
                }
            }
            _ = hub.wake.notified() => {
                dormant.clear();
            }
            _ = tokio::time::sleep(TUNING.tick) => {
                maintenance = true;
                dormant.clear();
            }
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(stopping),
    }
}

/// An agent that may be launchable.
struct Candidate {
    agent: String,
    role: Role,
    fleet: bool,
}

/// What one scheduling pass found.
struct PassView {
    candidates: Vec<Candidate>,
    /// Agents whose branch has a stop request.
    branch_stopped: HashSet<String>,
}

/// Recovers retained work when due, then lists agents that pass the cheap
/// claim gates. Launch tasks apply the transactional gates.
async fn pass(state: &SessionState, scan: &str, maintenance: bool) -> Result<PassView, TurnError> {
    let state = state.clone();
    let scan = scan.to_owned();
    tokio::task::spawn_blocking(move || {
        if maintenance {
            distributed::LocalWorkerHost::recover(&state, &scan)
                .map_err(|_| TurnError::Internal)?;
            distributed::local_allocation::reconcile(&state).map_err(|_| TurnError::Internal)?;
            state.swarm_scans.allocations_changed.notify_waiters();
        }
        let store = store(&state.db)?;
        let saved = store.load(&scan, true).map_err(|_| TurnError::Internal)?;
        let local = store.local_state(&scan).map_err(|_| TurnError::Internal)?;
        Ok(PassView {
            candidates: candidates(&saved, &local),
            branch_stopped: local
                .controls
                .iter()
                .filter(|control| control.stop_requested)
                .map(|control| control.agent_id.clone())
                .collect(),
        })
    })
    .await
    .map_err(|_| TurnError::Internal)?
}

fn executing(saved: &beale_storage::swarm_state::ScanState, agent: &str) -> bool {
    saved
        .attempts
        .iter()
        .any(|a| a.agent_id == agent && a.state != ExecutionState::Stopped)
}

/// Mirrors the cheap conditions of the local claim: a live control, no
/// running attempt, a committed parent release when one is pending, and
/// unread input or a queued state.
fn candidates(
    saved: &beale_storage::swarm_state::ScanState,
    local: &beale_storage::swarm_state::LocalState,
) -> Vec<Candidate> {
    if saved.ended_at_ms.is_some() {
        return Vec::new();
    }
    let fleet = !saved.config.destinations.is_empty();
    saved
        .agents
        .iter()
        .filter_map(|agent| {
            let id = agent.summary.id.as_str();
            let control = local.controls.iter().find(|c| c.agent_id == id)?;
            if control.stop_requested || control.failures >= 3 || executing(saved, id) {
                return None;
            }
            if let Some(parent) = &control.release_parent
                && (executing(saved, parent)
                    || !local
                        .controls
                        .iter()
                        .any(|c| &c.agent_id == parent && (c.yield_committed || c.failures >= 3)))
            {
                return None;
            }
            let unread = local
                .inbox
                .iter()
                .any(|m| m.to == id && m.sequence > agent.checkpoint.inbox_cursor);
            if !unread && agent.summary.state != AgentState::Queued {
                return None;
            }
            Some(Candidate {
                agent: id.to_owned(),
                role: agent.summary.role,
                fleet: fleet && agent.summary.role != Role::Leader,
            })
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn launch(
    state: SessionState,
    scan: String,
    workspace_id: String,
    candidate: Candidate,
    lock: Arc<Mutex<()>>,
    stop: watch::Receiver<bool>,
    events: broadcast::Sender<SessionStreamEvent>,
    workers: Arc<tokio::sync::OnceCell<distributed::LocalWorkerHost>>,
) -> Result<Launched, TurnError> {
    let store = store(&state.db)?;
    let snapshot = store.load(&scan, true).map_err(|_| TurnError::Internal)?;
    let agent = candidate.agent.as_str();
    if candidate.fleet {
        let ran = distributed::placement::run_pending(&state, &snapshot, agent, Some(stop))
            .await
            .map_err(|_| TurnError::Internal)?;
        return Ok(if ran { Launched::Ran } else { Launched::Idle });
    }
    let stop = Some(stop);
    let owner_key = Uuid::new_v4();
    let allocation = if candidate.role != Role::Leader {
        match distributed::local_allocation::reserve(&state, &snapshot, agent, &stop)
            .await
            .map_err(|error| {
                beale_host_log::error!("Native allocation pending: {error:#}");
                TurnError::Internal
            })? {
            Some(owned) => Some(owned),
            None => return Ok(Launched::Idle),
        }
    } else {
        None
    };
    let owner = distributed::acquire_local_execution_lock(&state, owner_key)
        .map_err(|_| TurnError::Internal)?
        .ok_or(TurnError::Internal)?;
    let Some(turn) = store
        .local_claim_owned(&scan, agent, now()?, 64, owner_key)
        .map_err(|_| TurnError::Internal)?
    else {
        distributed::discard_unclaimed_lock(&state, owner_key, owner);
        if let Some(allocation) = allocation {
            distributed::local_allocation::unclaimed(&state, &allocation)
                .map_err(|_| TurnError::Internal)?;
            state.swarm_scans.allocations_changed.notify_waiters();
        }
        return Ok(Launched::Idle);
    };
    if let Some(allocation) = &allocation {
        distributed::local_allocation::bind(&state, allocation, &turn)
            .map_err(|_| TurnError::Internal)?;
        state.swarm_scans.allocations_changed.notify_waiters();
    }
    wake(&state, &scan);
    if turn.role != Role::Leader {
        let host = match workers
            .get_or_try_init(|| distributed::LocalWorkerHost::start(state.clone()))
            .await
        {
            Ok(host) => host,
            Err(error) => {
                beale_host_log::error!("Swarm worker host startup failed: {error:#}");
                let record = store
                    .local_worker_record(&turn.attempt_id)
                    .map_err(|_| TurnError::Internal)?;
                store
                    .reconcile_unlaunched_worker(&record)
                    .map_err(|_| TurnError::Internal)?;
                return Ok(Launched::Ran);
            }
        };
        // Once launched, only the owning supervisor may release execution.
        // Failed persistence retains ownership and blocks replacement.
        if let Err(error) = host.run_owned(&turn, stop, owner_key, owner).await {
            beale_host_log::error!("Swarm assigned execution requires recovery: {error:#}");
        }
        return Ok(Launched::Ran);
    }
    run_leader(&state, &workspace_id, &snapshot, &turn, &lock, stop, events).await?;
    store
        .complete_coordinator(&turn.attempt_id, owner_key, now()?)
        .map_err(|_| TurnError::Internal)?;
    drop(owner);
    Ok(Launched::Ran)
}

/// Runs one claimed leader turn in process and records its outcome.
async fn run_leader(
    state: &SessionState,
    workspace_id: &str,
    snapshot: &beale_storage::swarm_state::ScanState,
    turn: &beale_storage::swarm_state::LocalTurn,
    lock: &Arc<Mutex<()>>,
    stop: Option<watch::Receiver<bool>>,
    events: broadcast::Sender<SessionStreamEvent>,
) -> Result<(), TurnError> {
    let store = store(&state.db)?;
    let settings = store
        .local_state(&turn.scan_id)
        .map_err(|_| TurnError::Internal)?
        .settings;
    let choices: Vec<_> = settings
        .models
        .iter()
        .filter(|m| m.profile == turn.profile)
        .collect();
    let ordinal = snapshot
        .agents
        .iter()
        .filter(|a| a.summary.profile.id == turn.profile)
        .position(|a| a.summary.id == turn.agent_id)
        .ok_or(TurnError::Internal)?;
    let model = choices
        .get(
            ordinal
                .checked_rem(choices.len())
                .ok_or(TurnError::Internal)?,
        )
        .ok_or(TurnError::Internal)?;
    let input = SubmitSessionTurnRequest {
        workspace_id: workspace_id.to_owned(),
        session_id: turn.session_id.clone(),
        prompt: turn.prompt.clone(),
        provider: model.provider,
        method: model.method,
        model: model.model.clone(),
        reasoning_effort: settings.reasoning_effort,
        ..Default::default()
    };
    let number = state
        .db
        .session(&turn.session_id)
        .map_err(|_| TurnError::Internal)?
        .and_then(|s| s.turns.last().map(|t| t.number))
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(TurnError::Internal)?;
    let tagged = TurnProgress::new(events, number as u64);
    let result = engine::run_one_turn(
        state,
        input,
        tagged.sender.clone(),
        turn.operator_text.is_none(),
        lock,
        stop.clone(),
        Some(turn),
    )
    .await;
    tagged.finish().await;
    match result {
        Ok(detail) => {
            let answer = detail.messages.last().map_or("", |m| m.text.as_str());
            if answer.starts_with(REVIEW_UNAVAILABLE_ANSWER)
                || answer.starts_with(INTERRUPTED_ANSWER)
            {
                store.local_failed(turn).map_err(|_| TurnError::Internal)?;
            } else if engine::save_checkpoint(true, || {
                let context = json!({"session_id":turn.session_id,"saved":true});
                research_files::checkpoint(state, turn, &context)?;
                store.local_finish(
                    turn,
                    json!({"session_id":turn.session_id,"saved":true}),
                    answer,
                    engine::stop_requested(&stop),
                )
            })
            .await
            .is_err()
            {
                store
                    .local_checkpoint_failed(turn)
                    .map_err(|_| TurnError::Internal)?;
            }
        }
        Err(TurnError::RecoveryExhausted) => {
            store
                .local_recovery_exhausted(turn)
                .map_err(|_| TurnError::Internal)?;
        }
        Err(TurnError::Checkpoint) => {
            store
                .local_checkpoint_failed(turn)
                .map_err(|_| TurnError::Internal)?;
        }
        Err(_) => {
            store.local_failed(turn).map_err(|_| TurnError::Internal)?;
        }
    }
    Ok(())
}

/// Publishes live status to attached streams: after changes, and on a
/// fallback interval. One publisher serves every subscriber of the scan.
async fn publish(state: SessionState, hub: Arc<ScanHub>, request: SubmitSessionTurnRequest) {
    // Initialize the optional warning store before the first read-only projection.
    if beale_storage::provider_accounting::AccountingStore::open(state.db.clone()).is_err() {
        return;
    }
    let Ok(started) = now() else {
        return;
    };
    let mut previous: Option<SessionDetail> = None;
    let mut subscribers = 0;
    loop {
        tokio::select! {
            _ = hub.publish.notified() => {
                tokio::time::sleep(TUNING.publish_debounce).await;
            }
            _ = tokio::time::sleep(TUNING.publish_fallback) => {}
        }
        let count = hub.events.receiver_count();
        if count == 0 {
            subscribers = 0;
            continue;
        }
        if count > subscribers {
            // A new subscriber receives a complete snapshot.
            previous = None;
        }
        subscribers = count;
        let detail = {
            let state = state.clone();
            let scan = hub.scan.clone();
            let request = request.clone();
            match tokio::task::spawn_blocking(move || {
                live_detail(&state, &scan, &request, started as u64)
            })
            .await
            {
                Ok(Ok(detail)) => detail,
                _ => continue,
            }
        };
        if previous.as_ref() == Some(&detail) {
            continue;
        }
        let event = SessionStreamEvent {
            kind: SessionStreamKind::SwarmUpdated as i32,
            detail: Some(detail.clone()),
            ..Default::default()
        };
        if session_stream_frame(&event).is_some() && hub.events.send(event).is_ok() {
            previous = Some(detail);
        }
    }
}

/// Ends the scan when its work is complete or it was stopped, and records a
/// placeholder transcript when no leader turn was saved.
async fn closeout(
    state: &SessionState,
    scan: &str,
    request: &SubmitSessionTurnRequest,
    stopped: bool,
) -> Result<SessionDetail, TurnError> {
    let store = store(&state.db)?;
    let saved = store.load(scan, true).map_err(|_| TurnError::Internal)?;
    if saved.ready_to_end().map_err(|_| TurnError::Internal)? || stopped {
        store
            .end_scan(scan, now()?)
            .map_err(|_| TurnError::Internal)?;
    }
    if let Err(error) = distributed::placement::sync_lifecycle(
        state,
        &store.load(scan, true).map_err(|_| TurnError::Internal)?,
    )
    .await
    {
        beale_host_log::error!("Scan retention update remains pending: {error:#}");
    }
    if state
        .db
        .session(scan)
        .map_err(|_| TurnError::Internal)?
        .is_none()
    {
        let time = now()?;
        let session = SessionRow {
            id: scan.to_owned(),
            workspace_id: request.workspace_id.clone(),
            title: title(&request.prompt),
            created_at_ms: time,
            last_message_at_ms: time,
        };
        let answer = if stopped {
            "Swarm stopped before its first completed turn."
        } else {
            "Swarm stopped after bounded local recovery failed. The scan state is retained."
        };
        state
            .db
            .append_turn(
                &session,
                &TurnRow {
                    number: 1,
                    provider: request.provider,
                    method: request.method,
                    model: request.model.clone(),
                    prompt: request.prompt.clone(),
                    answer: answer.into(),
                    duration_ms: 0,
                    internal_prompt: false,
                },
                true,
            )
            .map_err(|_| TurnError::Internal)?;
    }
    beale_storage::provider_accounting::AccountingStore::open(state.db.clone())
        .map_err(|_| TurnError::Internal)?;
    live_detail(state, scan, request, now()? as u64).map_err(Into::into)
}
