//! Leader-owned peer execution intents. Credentials and host paths are absent.
use super::*;
use beale_contracts::{
    swarm_host::{ExecutionTarget, HostExecutionPhase, HostExecutionReport},
    swarm_worker::{ExecutionIdentity, WorkerShell},
};

pub(super) const SCHEMA: &str = "CREATE TABLE swarm_peer_workers (
 attempt TEXT PRIMARY KEY, scan TEXT NOT NULL REFERENCES swarm_scans(id),
 owner TEXT NOT NULL UNIQUE, complete INTEGER NOT NULL CHECK(complete IN (0,1)), document TEXT NOT NULL
);";

/// Host-selected placement after admission; never constructed from model input.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerPlacement {
    pub leader_id: String,
    pub host_id: String,
    pub configuration_id: String,
    pub configuration_revision: u64,
    pub instance_id: String,
    pub shell: WorkerShell,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerWorkerRecord {
    pub turn: LocalTurn,
    pub target: ExecutionTarget,
    pub shell: WorkerShell,
    pub owner_key: Uuid,
    pub dispatched: bool,
    pub stop_requested: bool,
    pub report: Option<HostExecutionReport>,
    pub disposition: Option<WorkerDisposition>,
    pub complete: bool,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerRecoveryWarning {
    PairingUnavailable,
    CheckpointPending,
}
impl PeerRecoveryWarning {
    fn description(self) -> &'static str {
        match self {
            Self::PairingUnavailable => {
                "Remote execution recovery cannot reach its saved host pairing. Authority is fenced and replacement remains blocked."
            }
            Self::CheckpointPending => {
                "Remote execution has stopped; its private workspace checkpoint is pending. Replacement remains blocked."
            }
        }
    }
}
impl PeerWorkerRecord {
    pub fn terminated(&self) -> bool {
        self.report
            .as_ref()
            .is_some_and(|r| r.phase == HostExecutionPhase::Stopped)
    }
}
pub(super) fn read(db: &Connection, attempt: &str) -> Result<PeerWorkerRecord> {
    let (scan, owner, complete, document): (String, String, bool, String) = db.query_row(
        "SELECT scan,owner,complete,document FROM swarm_peer_workers WHERE attempt=?1",
        [attempt],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let record: PeerWorkerRecord = serde_json::from_str(&document)?;
    record.target.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        record.turn.attempt_id == attempt
            && record.turn.scan_id == scan
            && record.owner_key.to_string() == owner
            && record.complete == complete
            && record.turn.role != Role::Leader
            && record.target.identity.scan_id == scan
            && record.target.identity.attempt_id == attempt
            && record.target.identity.agent_id == record.turn.agent_id
            && record.target.identity.generation == record.turn.generation,
        "invalid peer execution identity"
    );
    let state = read_scan(db, &record.turn.scan_id)?;
    ensure!(
        state.config.workspace_id == record.target.identity.workspace_id
            && state.attempts.iter().any(|a| a.id == attempt
                && a.agent_id == record.turn.agent_id
                && a.generation == record.turn.generation),
        "peer record does not belong to saved execution"
    );
    if let Some(report) = &record.report {
        report.validate().map_err(anyhow::Error::msg)?;
        ensure!(
            report.target == record.target,
            "foreign peer execution report"
        );
    }
    ensure!(
        !record.complete || record.disposition.is_some(),
        "peer completion has no disposition"
    );
    Ok(record)
}
fn owned(db: &Connection, attempt: &str, key: Uuid) -> Result<PeerWorkerRecord> {
    let record = read(db, attempt)?;
    ensure!(record.owner_key == key, "peer execution owner mismatch");
    Ok(record)
}
fn write(tx: &Transaction<'_>, record: &PeerWorkerRecord) -> Result<()> {
    ensure!(
        tx.execute(
            "UPDATE swarm_peer_workers SET document=?2,complete=?3 WHERE attempt=?1 AND owner=?4",
            params![
                record.turn.attempt_id,
                serde_json::to_string(record)?,
                record.complete,
                record.owner_key.to_string()
            ]
        )? == 1,
        "peer execution ownership changed"
    );
    Ok(())
}
pub(super) fn register_in(
    tx: &Transaction<'_>,
    turn: &LocalTurn,
    key: Uuid,
    placement: &PeerPlacement,
) -> Result<()> {
    let mut state = read_scan(tx, &turn.scan_id)?;
    local::ensure_local_turn(&state, turn)?;
    ensure!(
        turn.role != Role::Leader
            && state
                .config
                .destinations
                .iter()
                .any(|d| d.host_id == placement.host_id
                    && d.configuration_id == placement.configuration_id),
        "peer placement was not selected for this scan"
    );
    let target = ExecutionTarget {
        host_id: placement.host_id.clone(),
        leader_id: placement.leader_id.clone(),
        configuration_id: placement.configuration_id.clone(),
        configuration_revision: placement.configuration_revision,
        instance_id: placement.instance_id.clone(),
        identity: ExecutionIdentity {
            workspace_id: state.config.workspace_id.clone(),
            scan_id: turn.scan_id.clone(),
            agent_id: turn.agent_id.clone(),
            attempt_id: turn.attempt_id.clone(),
            generation: turn.generation,
        },
    };
    target.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM swarm_worker_runtime WHERE attempt_id=?1)",
            [&turn.attempt_id],
            |r| r.get::<_, bool>(0)
        )?,
        "peer already prepared without intent"
    );
    if let Some(instance) = state
        .instances
        .iter_mut()
        .find(|i| i.id == placement.instance_id)
    {
        ensure!(
            instance.host_id == placement.host_id
                && instance.configuration_id == placement.configuration_id
                && instance.agent_id == turn.agent_id
                && instance.state == InstanceState::Stopped,
            "retained instance belongs to other work"
        );
        let previous = read(tx, &instance.attempt_id)?;
        ensure!(
            previous.target.configuration_revision == placement.configuration_revision
                && previous.target.leader_id == placement.leader_id
                && previous.complete,
            "retained peer instance is not reconciled"
        );
        instance.attempt_id = turn.attempt_id.clone();
        instance.state = InstanceState::Starting;
    } else {
        tx.execute("INSERT INTO swarm_instance_owners(host_id,instance_id,scan_id,agent_id) VALUES(?1,?2,?3,?4)",
            params![placement.host_id,placement.instance_id,turn.scan_id,turn.agent_id])?;
        state.instances.push(ExecutionInstance {
            id: placement.instance_id.clone(),
            host_id: placement.host_id.clone(),
            configuration_id: placement.configuration_id.clone(),
            agent_id: turn.agent_id.clone(),
            attempt_id: turn.attempt_id.clone(),
            state: InstanceState::Starting,
        });
    }
    write_scan(tx, &mut state)?;
    let record = PeerWorkerRecord {
        turn: turn.clone(),
        target,
        shell: placement.shell,
        owner_key: key,
        dispatched: false,
        stop_requested: false,
        report: None,
        disposition: None,
        complete: false,
    };
    tx.execute(
        "INSERT INTO swarm_peer_workers VALUES(?1,?2,?3,0,?4)",
        params![
            turn.attempt_id,
            turn.scan_id,
            key.to_string(),
            serde_json::to_string(&record)?
        ],
    )?;
    Ok(())
}

impl SwarmStore {
    /// Host-owned, bounded warnings; no endpoint, token, or filesystem error is
    /// copied into the session. Retrying the same reason does not duplicate it.
    pub fn peer_recovery_warning(
        &self,
        attempt: &str,
        key: Uuid,
        reason: PeerRecoveryWarning,
    ) -> Result<()> {
        self.transaction(|tx| {
            let record = owned(tx, attempt, key)?;
            if record.complete { return Ok(()); }
            ensure!(
                !matches!(reason, PeerRecoveryWarning::CheckpointPending) || record.terminated(),
                "checkpoint warning requires confirmed termination"
            );
            let mut state = read_scan(tx, &record.turn.scan_id)?;
            let warning = reason.description();
            let reason = serde_json::to_value(reason)?;
            let id = format!("peer-recovery:{attempt}:{}", reason.as_str().context("invalid recovery reason")?);
            if state.events.iter().any(|event| event.id == id) { return Ok(()); }
            state.events.push(LifecycleEvent {
                id,
                agent_id: record.turn.agent_id.clone(),
                sequence: next(state.events.last().map_or(0, |event| event.sequence))?,
                kind: "peer-recovery-pending".into(),
                payload: json!({"attempt_id":attempt,"generation":record.turn.generation,"reason":reason,"warning":warning}),
            });
            write_scan(tx, &mut state)
        })
    }

    pub fn peer_claim_owned(
        &self,
        scan: &str,
        agent: &str,
        now: i64,
        capacity: usize,
        key: Uuid,
        placement: &PeerPlacement,
    ) -> Result<Option<LocalTurn>> {
        self.local_claim_inner(
            scan,
            agent,
            now,
            capacity,
            Some(local::ClaimOwner::Peer(key, placement)),
        )
    }
    pub fn peer_worker(&self, attempt: &str) -> Result<PeerWorkerRecord> {
        self.db.with_connection(|db| read(db, attempt))
    }
    pub fn abandon_unprepared_peer(&self, attempt: &str, key: Uuid) -> Result<()> {
        self.transaction(|tx| {
            let mut record = owned(tx, attempt, key)?;
            if record.complete {
                return Ok(());
            }
            ensure!(!record.dispatched, "peer launch may have been delivered");
            local_workers::abandon_unprepared_in(tx, &record.turn)?;
            fence_in_transaction(tx, &record.turn.scan_id, attempt, true)?;
            record.disposition = Some(WorkerDisposition::Failed);
            record.complete = true;
            write(tx, &record)
        })
    }
    pub fn peer_recovery_page(&self, after: &str) -> Result<Vec<String>> {
        self.db.with_connection(|db| {
            let mut stmt = db.prepare("SELECT attempt FROM swarm_peer_workers WHERE complete=0 AND attempt>?1 ORDER BY attempt LIMIT 64")?;
            Ok(stmt.query_map([after],|r|r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
    /// Commit before sending Start. Reopened owners never call this operation.
    pub fn peer_dispatch(&self, attempt: &str, key: Uuid) -> Result<PeerWorkerRecord> {
        self.transaction(|tx| {
            let mut record = owned(tx, attempt, key)?;
            ensure!(
                !record.complete && !record.stop_requested && !record.dispatched,
                "peer launch already dispatched or stopped"
            );
            let state = read_scan(tx, &record.turn.scan_id)?;
            local::ensure_local_turn(&state, &record.turn)?;
            ensure_worker_branch_open(tx, &record.turn.scan_id, &record.turn.agent_id)?;
            ensure!(
                worker_runtime::provenance_in(tx, attempt)?.identity == record.target.identity,
                "peer bootstrap mismatch"
            );
            record.dispatched = true;
            write(tx, &record)?;
            Ok(record)
        })
    }
    /// Sticky stop intent commits with credential fencing before peer IO.
    pub fn peer_request_stop(&self, attempt: &str, key: Uuid) -> Result<PeerWorkerRecord> {
        self.transaction(|tx| {
            let mut record = owned(tx, attempt, key)?;
            if !record.complete {
                record.stop_requested = true;
                fence_in_transaction(tx, &record.turn.scan_id, attempt, false)?;
                write(tx, &record)?;
            }
            Ok(record)
        })
    }
    pub fn peer_observe(
        &self,
        attempt: &str,
        key: Uuid,
        report: &HostExecutionReport,
    ) -> Result<PeerWorkerRecord> {
        report.validate().map_err(anyhow::Error::msg)?;
        self.transaction(|tx| {
            let mut record = owned(tx, attempt, key)?;
            ensure!(
                report.target == record.target,
                "peer report identity mismatch"
            );
            if let Some(previous) = &record.report {
                if report.revision < previous.revision {
                    return Ok(record);
                }
                if report.revision == previous.revision {
                    ensure!(report == previous, "peer report revision changed");
                    return Ok(record);
                }
                ensure!(
                    previous.phase != HostExecutionPhase::Stopped
                        || report.phase == HostExecutionPhase::Stopped,
                    "terminated peer revived"
                );
                ensure!(
                    !previous.stop_requested || report.stop_requested,
                    "peer cancellation regressed"
                );
            }
            record.report = Some(report.clone());
            record.stop_requested |= report.stop_requested;
            if record.terminated() {
                fence_in_transaction(tx, &record.turn.scan_id, attempt, false)?;
            }
            write(tx, &record)?;
            Ok(record)
        })
    }
    /// Freeze the reconciliation identity before any filesystem checkpoint.
    pub fn peer_disposition(
        &self,
        attempt: &str,
        key: Uuid,
        disposition: WorkerDisposition,
    ) -> Result<WorkerDisposition> {
        self.transaction(|tx| {
            let mut record = owned(tx, attempt, key)?;
            ensure!(record.terminated(), "peer termination is unconfirmed");
            if let Some(saved) = record.disposition {
                return Ok(saved);
            }
            record.disposition = Some(disposition);
            write(tx, &record)?;
            Ok(disposition)
        })
    }
    pub fn complete_peer_worker(&self, attempt: &str, key: Uuid) -> Result<()> {
        self.transaction(|tx| {
            let mut record = owned(tx, attempt, key)?;
            let disposition = record.disposition.context("peer disposition missing")?;
            ensure!(record.terminated(), "peer termination unconfirmed");
            let result = receipt(
                tx,
                &record.turn.scan_id,
                "supervisor",
                &format!("reconcile:{attempt}"),
                &serde_json::to_value(disposition)?,
            )?
            .context("peer canonical reconciliation missing")?;
            let result: WorkerReconciliation = serde_json::from_value(result)?;
            ensure!(
                result != WorkerReconciliation::TerminationUnconfirmed,
                "peer canonical reconciliation incomplete"
            );
            ensure!(
                read_scan(tx, &record.turn.scan_id)?
                    .attempts
                    .iter()
                    .any(|a| a.id == attempt && a.state == ExecutionState::Stopped),
                "peer execution still owned"
            );
            record.complete = true;
            write(tx, &record)
        })
    }
}
