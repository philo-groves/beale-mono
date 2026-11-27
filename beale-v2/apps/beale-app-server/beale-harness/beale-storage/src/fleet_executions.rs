//! Fleet-owned execution intents. These contain no provider or worker secrets.
//! Only an owning host backend may attest process termination.
use crate::HostDatabase;
use anyhow::{Context, Result, ensure};
use beale_contracts::swarm_host::*;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub struct ExecutionConflict(pub &'static str);
impl std::fmt::Display for ExecutionConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ExecutionConflict {}
fn require(condition: bool, message: &'static str) -> Result<()> {
    if !condition {
        return Err(ExecutionConflict(message).into());
    }
    Ok(())
}

const SCHEMA: &str = "
CREATE TABLE fleet_execution_identity(singleton INTEGER PRIMARY KEY CHECK(singleton=1), id TEXT NOT NULL);
CREATE TABLE fleet_execution_instances(instance TEXT PRIMARY KEY, owner TEXT NOT NULL);
CREATE TABLE fleet_executions(
 leader TEXT NOT NULL, attempt TEXT NOT NULL, workspace TEXT NOT NULL, scan TEXT NOT NULL,
 agent TEXT NOT NULL, generation INTEGER NOT NULL, instance TEXT NOT NULL,
 target TEXT NOT NULL, launch_hash TEXT, phase TEXT NOT NULL, revision INTEGER NOT NULL,
 stop_requested INTEGER NOT NULL CHECK(stop_requested IN (0,1)),
 PRIMARY KEY(leader,attempt), UNIQUE(leader,workspace,scan,agent,generation));
CREATE UNIQUE INDEX fleet_execution_live_agent ON fleet_executions(leader,workspace,scan,agent) WHERE phase != 'stopped';
CREATE UNIQUE INDEX fleet_execution_live_instance ON fleet_executions(instance) WHERE phase != 'stopped';
CREATE TABLE fleet_execution_operations(leader TEXT NOT NULL, operation TEXT NOT NULL, fingerprint TEXT NOT NULL,
 PRIMARY KEY(leader,operation));
";

#[derive(Clone)]
pub struct FleetExecutionStore {
    db: HostDatabase,
}

pub struct ExecutionAcceptance {
    pub report: HostExecutionReport,
    /// True exactly once, after the durable launch claim commits. Reconnect
    /// observes existing execution; it never repeats an uncertain launch.
    pub launch: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionCursor {
    pub leader: String,
    pub attempt: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendObservation {
    Running,
    TerminationUnconfirmed,
    Stopped,
}

impl FleetExecutionStore {
    pub fn open(db: HostDatabase) -> Result<Self> {
        db.migrate_schema("fleet-executions", &[SCHEMA])?;
        let result = Self { db };
        result.transaction(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO fleet_execution_identity VALUES(1,?1)",
                [uuid::Uuid::new_v4().to_string()],
            )?;
            Ok(())
        })?;
        Ok(result)
    }
    fn transaction<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        self.db.with_connection(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let result = f(&tx)?;
            tx.commit()?;
            Ok(result)
        })
    }
    pub fn host_id(&self) -> Result<String> {
        self.db.with_connection(|db| {
            Ok(db.query_row(
                "SELECT id FROM fleet_execution_identity WHERE singleton=1",
                [],
                |row| row.get(0),
            )?)
        })
    }
    /// Page identities independently so a corrupt launch cannot starve later
    /// records. This contains no launch credentials or backend paths.
    pub fn pending_page(&self, after: Option<&ExecutionCursor>) -> Result<Vec<ExecutionCursor>> {
        self.db.with_connection(|db| {
            let (leader, attempt) = after.map_or(("", ""), |cursor| (cursor.leader.as_str(), cursor.attempt.as_str()));
            let mut query = db.prepare("SELECT leader,attempt FROM fleet_executions WHERE phase!='stopped' AND (leader,attempt)>(?1,?2) ORDER BY leader,attempt LIMIT 64")?;
            Ok(query.query_map(params![leader,attempt], |row| Ok(ExecutionCursor { leader: row.get(0)?, attempt: row.get(1)? }))?.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
    pub fn pending(&self, cursor: &ExecutionCursor) -> Result<HostExecutionReport> {
        self.transaction(|tx| {
            let document: String = tx.query_row(
                "SELECT target FROM fleet_executions WHERE leader=?1 AND attempt=?2",
                params![cursor.leader, cursor.attempt],
                |row| row.get(0),
            )?;
            let target: ExecutionTarget = serde_json::from_str(&document)?;
            target.validate().map_err(anyhow::Error::msg)?;
            ensure!(
                target.leader_id == cursor.leader && target.identity.attempt_id == cursor.attempt,
                "corrupt pending execution identity"
            );
            check_host(tx, &target)?;
            load(tx, &target)?.context("missing pending execution")
        })
    }
    pub fn inspect(&self, target: &ExecutionTarget) -> Result<Option<HostExecutionReport>> {
        target.validate().map_err(anyhow::Error::msg)?;
        self.transaction(|tx| {
            check_host(tx, target)?;
            load(tx, target)
        })
    }
    pub fn accept(&self, request: &HostExecutionRequest) -> Result<ExecutionAcceptance> {
        request.validate().map_err(anyhow::Error::msg)?;
        self.transaction(|tx| {
            let target = &request.target;
            check_host(tx, target)?;
            let saved = load(tx, target)?;
            if matches!(request.operation, HostExecutionOperation::Inspect {}) {
                return Ok(ExecutionAcceptance { report: saved.context("unknown host execution")?, launch: false });
            }
            let fingerprint = hash(request)?;
            let old: Option<String> = tx.query_row("SELECT fingerprint FROM fleet_execution_operations WHERE leader=?1 AND operation=?2",
                params![target.leader_id, request.operation_id], |row| row.get(0)).optional()?;
            if let Some(old) = old {
                require(old == fingerprint, "host operation identity changed")?;
                return Ok(ExecutionAcceptance { report: saved.context("missing accepted execution")?, launch: false });
            }
            let launch_hash = match &request.operation {
                HostExecutionOperation::Start { launch } => Some(hash(launch)?),
                _ => None,
            };
            let mut launch = false;
            let report = if let Some(mut report) = saved {
                if let Some(fingerprint) = &launch_hash {
                    let old: Option<String> = tx.query_row("SELECT launch_hash FROM fleet_executions WHERE leader=?1 AND attempt=?2",
                        params![target.leader_id, target.identity.attempt_id], |row| row.get(0))?;
                    // A stop-before-start tombstone has no launch fingerprint.
                    require(old.is_none_or(|old| old == *fingerprint), "host launch content changed")?;
                }
                if matches!(request.operation, HostExecutionOperation::Stop {}) && report.phase != HostExecutionPhase::Stopped {
                    report.phase = HostExecutionPhase::StopRequested;
                    report.stop_requested = true;
                    save(tx, &mut report)?;
                }
                report
            } else {
                // Retained instances belong to one logical agent, including
                // after completion. New turns may reuse that agent's instance.
                let owner = serde_json::to_string(&( &target.leader_id, &target.identity.workspace_id,
                    &target.identity.scan_id, &target.identity.agent_id, &target.configuration_id, target.configuration_revision ))?;
                let old: Option<String> = tx.query_row("SELECT owner FROM fleet_execution_instances WHERE instance=?1", [&target.instance_id], |row| row.get(0)).optional()?;
                require(old.as_ref().is_none_or(|old| old == &owner), "instance belongs to another agent or configuration")?;
                let generation_exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM fleet_executions WHERE leader=?1 AND workspace=?2 AND scan=?3 AND agent=?4 AND generation=?5)",
                    params![target.leader_id,target.identity.workspace_id,target.identity.scan_id,target.identity.agent_id,i64::try_from(target.identity.generation)?], |row|row.get(0))?;
                require(!generation_exists, "execution generation already has an attempt")?;
                if launch_hash.is_some() {
                    let occupied: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM fleet_executions WHERE phase!='stopped' AND (instance=?1 OR (leader=?2 AND workspace=?3 AND scan=?4 AND agent=?5)))",
                        params![target.instance_id,target.leader_id,target.identity.workspace_id,target.identity.scan_id,target.identity.agent_id], |row|row.get(0))?;
                    require(!occupied, "execution still owns the agent or instance")?;
                }
                tx.execute("INSERT OR IGNORE INTO fleet_execution_instances VALUES(?1,?2)", params![target.instance_id, owner])?;
                launch = launch_hash.is_some();
                let phase = if launch { HostExecutionPhase::LaunchRequested } else { HostExecutionPhase::Stopped };
                let report = HostExecutionReport { target: target.clone(), phase, revision: 1, stop_requested: !launch };
                tx.execute("INSERT INTO fleet_executions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1,?11)",
                    params![target.leader_id, target.identity.attempt_id, target.identity.workspace_id, target.identity.scan_id,
                        target.identity.agent_id, i64::try_from(target.identity.generation)?, target.instance_id, serde_json::to_string(target)?,
                        launch_hash, phase_name(phase), !launch])?;
                report
            };
            tx.execute("INSERT INTO fleet_execution_operations VALUES(?1,?2,?3)", params![target.leader_id, request.operation_id, fingerprint])?;
            Ok(ExecutionAcceptance { report, launch })
        })
    }
    /// A stale observation cannot clear a stop request or overwrite completion.
    /// Lost acknowledgements can re-read the latest revision and reconcile again.
    pub fn observe(
        &self,
        target: &ExecutionTarget,
        expected_revision: u64,
        observation: BackendObservation,
    ) -> Result<HostExecutionReport> {
        target.validate().map_err(anyhow::Error::msg)?;
        self.transaction(|tx| {
            check_host(tx, target)?;
            let mut report = load(tx, target)?.context("unknown host execution")?;
            if report.revision != expected_revision || report.phase == HostExecutionPhase::Stopped {
                return Ok(report);
            }
            let phase = match observation {
                BackendObservation::Stopped => HostExecutionPhase::Stopped,
                BackendObservation::Running if report.stop_requested => {
                    HostExecutionPhase::StopRequested
                }
                BackendObservation::Running => HostExecutionPhase::Running,
                BackendObservation::TerminationUnconfirmed => {
                    HostExecutionPhase::TerminationUnconfirmed
                }
            };
            if report.phase != phase {
                report.phase = phase;
                save(tx, &mut report)?;
            }
            Ok(report)
        })
    }
}

fn hash(value: &impl serde::Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn check_host(tx: &Transaction<'_>, target: &ExecutionTarget) -> Result<()> {
    let id: String = tx.query_row(
        "SELECT id FROM fleet_execution_identity WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    require(id == target.host_id, "wrong execution host")
}
fn load(tx: &Transaction<'_>, target: &ExecutionTarget) -> Result<Option<HostExecutionReport>> {
    let row: Option<(String, String, i64, bool)> = tx
        .query_row(
            "SELECT target,phase,revision,stop_requested FROM fleet_executions WHERE leader=?1 AND attempt=?2",
            params![target.leader_id, target.identity.attempt_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    row.map(|(saved, phase, revision, stop_requested)| {
        let saved: ExecutionTarget = serde_json::from_str(&saved)?;
        require(saved == *target, "host execution identity changed")?;
        ensure!(revision > 0, "invalid host execution revision");
        let phase = serde_json::from_value(serde_json::Value::String(phase))?;
        let report = HostExecutionReport {
            target: saved,
            phase,
            revision: u64::try_from(revision)?,
            stop_requested,
        };
        report.validate().map_err(anyhow::Error::msg)?;
        Ok(report)
    })
    .transpose()
}
fn phase_name(phase: HostExecutionPhase) -> &'static str {
    match phase {
        HostExecutionPhase::LaunchRequested => "launch_requested",
        HostExecutionPhase::Running => "running",
        HostExecutionPhase::StopRequested => "stop_requested",
        HostExecutionPhase::TerminationUnconfirmed => "termination_unconfirmed",
        HostExecutionPhase::Stopped => "stopped",
    }
}
fn save(tx: &Transaction<'_>, report: &mut HostExecutionReport) -> Result<()> {
    report.revision = report
        .revision
        .checked_add(1)
        .filter(|n| *n <= i64::MAX as u64)
        .context("host execution revision overflow")?;
    tx.execute(
        "UPDATE fleet_executions SET phase=?3,revision=?4,stop_requested=?5 WHERE leader=?1 AND attempt=?2",
        params![
            report.target.leader_id,
            report.target.identity.attempt_id,
            phase_name(report.phase),
            i64::try_from(report.revision)?,
            report.stop_requested
        ],
    )?;
    Ok(())
}
