//! Atomic owning-host reservations, fair admission, and retained-use claims.
use crate::{
    HostDatabase,
    rusqlite::{OptionalExtension, Transaction, params},
};
use anyhow::{Context, Result, ensure};
use beale_contracts::swarm_allocation::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct AllocationStore {
    db: HostDatabase,
}
impl AllocationStore {
    pub fn open(db: HostDatabase) -> Result<Self> {
        db.migrate_schema("fleet-allocation", &["CREATE TABLE fleet_allocations(id TEXT PRIMARY KEY, sequence INTEGER NOT NULL UNIQUE, document TEXT NOT NULL); CREATE TABLE fleet_allocation_fairness(participant TEXT PRIMARY KEY, served INTEGER NOT NULL); CREATE TABLE fleet_retention(instance TEXT PRIMARY KEY, document TEXT NOT NULL);"])?;
        db.migrate_schema("fleet-allocation-specs", &["CREATE TABLE fleet_allocation_specs(id TEXT PRIMARY KEY, specification TEXT NOT NULL, request TEXT NOT NULL);"])?;
        db.migrate_schema("fleet-allocation-lifecycle", &["CREATE TABLE fleet_allocation_lifecycle(leader TEXT NOT NULL,scan TEXT NOT NULL,document TEXT NOT NULL,PRIMARY KEY(leader,scan));"])?;
        db.migrate_schema("fleet-allocation-archives", &["CREATE TABLE fleet_allocation_archives(id TEXT PRIMARY KEY,instance TEXT NOT NULL);"])?;
        db.migrate_schema(
            "fleet-export-cancellations",
            &["CREATE TABLE fleet_export_cancellations(target TEXT NOT NULL, transfer TEXT NOT NULL, PRIMARY KEY(target,transfer));"],
        )?;
        db.migrate_schema("fleet-retention-discards", &["CREATE TABLE fleet_retention_discards(confirmation TEXT PRIMARY KEY,document TEXT NOT NULL,expires INTEGER NOT NULL,confirmed INTEGER NOT NULL DEFAULT 0);"])?;
        db.migrate_schema(
            "fleet-allocation-cancellations",
            &["CREATE TABLE fleet_allocation_cancellations(id TEXT PRIMARY KEY);"],
        )?;
        Ok(Self { db })
    }
    fn transaction<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        self.db.with_connection(|db| {
            let tx =
                db.transaction_with_behavior(crate::rusqlite::TransactionBehavior::Immediate)?;
            let result = f(&tx)?;
            tx.commit()?;
            Ok(result)
        })
    }
    pub fn allocations(&self) -> Result<Vec<Allocation>> {
        self.db.with_connection(|db| read_all(db))
    }
    pub fn bind_archive(&self, id: &str, instance: &str) -> Result<()> {
        ensure!(
            beale_contracts::swarm_files::digest(id),
            "invalid archive identity"
        );
        self.transaction(|tx| {
            let environment = retained(tx, instance)?;
            ensure!(
                environment.deletion == DeletionStep::Retained,
                "archive environment deletion claimed"
            );
            let old: Option<String> = tx
                .query_row(
                    "SELECT instance FROM fleet_allocation_archives WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?;
            ensure!(
                old.as_deref().is_none_or(|old| old == instance),
                "archive ownership changed"
            );
            tx.execute(
                "INSERT OR IGNORE INTO fleet_allocation_archives VALUES(?1,?2)",
                params![id, instance],
            )?;
            Ok(())
        })
    }
    pub fn archive_ids(&self, instance: &str) -> Result<Vec<String>> {
        self.db.with_connection(|db| {
            let mut q = db.prepare(
                "SELECT id FROM fleet_allocation_archives WHERE instance=?1 ORDER BY id",
            )?;
            Ok(q.query_map([instance], |r| r.get(0))?
                .collect::<crate::rusqlite::Result<Vec<_>>>()?)
        })
    }
    pub fn export_canceled(
        &self,
        target: &beale_contracts::swarm_host::ExecutionTarget,
        transfer: &str,
    ) -> Result<bool> {
        self.db.with_connection(|db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM fleet_export_cancellations WHERE target=?1 AND transfer=?2)",
                params![serde_json::to_string(target)?, transfer],
                |r| r.get(0),
            )?)
        })
    }
    pub fn cancel_export(
        &self,
        target: &beale_contracts::swarm_host::ExecutionTarget,
        transfer: &str,
    ) -> Result<()> {
        target.validate().map_err(anyhow::Error::msg)?;
        self.db.with_connection(|db| {
            db.execute(
                "INSERT OR IGNORE INTO fleet_export_cancellations VALUES(?1,?2)",
                params![serde_json::to_string(target)?, transfer],
            )?;
            Ok(())
        })
    }
    /// Only an unstarted, live reservation may renew its transfer lease. The
    /// leader bounds the whole transfer; abandoned work still expires.
    pub fn renew_transfer(&self, id: &str, now: i64) -> Result<()> {
        self.transaction(|tx| {
            let mut row = read(tx, id)?;
            ensure!(
                row.phase == AllocationPhase::Reserved
                    && row.execution.is_none()
                    && row.reservation_deadline_ms >= now,
                "transfer reservation expired or started"
            );
            row.reservation_deadline_ms = now
                .checked_add(660_000)
                .context("transfer deadline overflow")?;
            write(tx, &row)
        })
    }
    /// Stopped private bytes and prepared shared images are already reflected
    /// in the owning filesystem's available-byte measurement.
    pub fn reflected_storage_keys(&self) -> Result<BTreeSet<String>> {
        Ok(self
            .allocations()?
            .into_iter()
            .flat_map(|a| {
                a.request
                    .storage
                    .into_iter()
                    .filter(move |d| {
                        a.phase == AllocationPhase::Retained
                            || a.prepared && d.key.starts_with("image-")
                    })
                    .map(|d| d.key)
            })
            .collect())
    }
    pub fn scan_lifecycle(&self, lifecycle: &ScanLifecycle, now: i64) -> Result<()> {
        ensure!(
            beale_contracts::swarms::valid_id(&lifecycle.leader)
                && beale_contracts::swarms::valid_id(&lifecycle.scan)
                && lifecycle.revision > 0
                && lifecycle.retention_seconds <= 365 * 86400
                && lifecycle.ended_at_ms.is_none_or(|n| (0..=now).contains(&n)),
            "invalid scan lifecycle"
        );
        self.transaction(|tx| {
            let old:Option<String>=tx.query_row("SELECT document FROM fleet_allocation_lifecycle WHERE leader=?1 AND scan=?2",params![lifecycle.leader,lifecycle.scan],|r|r.get(0)).optional()?;
            if let Some(old)=old {let old:ScanLifecycle=serde_json::from_str(&old)?;if old.revision>lifecycle.revision {return Ok(());} else if old.revision==lifecycle.revision {ensure!(old==*lifecycle,"scan lifecycle revision reused");return Ok(());}}
            let mut q=tx.prepare("SELECT instance FROM fleet_retention WHERE json_extract(document,'$.leader')=?1 AND json_extract(document,'$.scan')=?2")?;
            let ids=q.query_map(params![lifecycle.leader,lifecycle.scan],|r|r.get::<_,String>(0))?.collect::<crate::rusqlite::Result<Vec<_>>>()?;drop(q);
            for id in ids {
                let mut environment=retained(tx,&id)?;
                if lifecycle.ended_at_ms.is_none() {ensure!(environment.deletion==DeletionStep::Retained,"reopen conflicts with private environment deletion");}
                if environment.deletion!=DeletionStep::Retained {continue;}
                environment.ended_at_ms=lifecycle.ended_at_ms;
                environment.deadline_ms=lifecycle.ended_at_ms.map(|n|n.checked_add(i64::try_from(lifecycle.retention_seconds)?.checked_mul(1000).context("retention overflow")?).context("deadline overflow")).transpose()?;
                environment.generation=next(environment.generation)?;save_retained(tx,&environment)?;
            }
            tx.execute("INSERT INTO fleet_allocation_lifecycle VALUES(?1,?2,?3) ON CONFLICT(leader,scan) DO UPDATE SET document=excluded.document",params![lifecycle.leader,lifecycle.scan,serde_json::to_string(lifecycle)?])?;Ok(())
        })
    }
    pub fn allocation(&self, id: &str) -> Result<Allocation> {
        self.db.with_connection(|db| read(db, id))
    }
    pub fn bind_canonical_storage(
        &self,
        id: &str,
        target: &beale_contracts::swarm_host::ExecutionTarget,
    ) -> Result<()> {
        self.transaction(|tx| {
            let mut row = read(tx, id)?;
            ensure!(
                row.request.domain == "canonical-storage"
                    && row.phase == AllocationPhase::Reserved
                    && row.request.scan == target.identity.scan_id
                    && row.request.agent == target.identity.agent_id
                    && row.request.leader == target.leader_id
                    && row.execution.as_ref().is_none_or(|old| old == target),
                "canonical reservation execution mismatch"
            );
            row.execution = Some(target.clone());
            write(tx, &row)
        })
    }
    /// Canonical evidence outlives its incremental write reservation. Removing
    /// this reservation never removes any published byte or research record.
    pub fn release_canonical_storage(&self, id: &str) -> Result<()> {
        self.transaction(|tx| {
            let mut row = read(tx, id)?;
            ensure!(
                row.request.domain == "canonical-storage",
                "not a canonical storage reservation"
            );
            row.phase = AllocationPhase::Canceled;
            write(tx, &row)?;
            tx.execute(
                "DELETE FROM fleet_retention WHERE instance=?1",
                [&row.request.instance],
            )?;
            Ok(())
        })
    }
    pub fn allocation_if_present(&self, id: &str) -> Result<Option<Allocation>> {
        self.db.with_connection(|db| optional(db, id))
    }
    /// Freeze all client intent and host-derived choices before asynchronous
    /// preparation. Lost replies and concurrent retries share one instance.
    pub fn bind_spec(
        &self,
        spec: &AllocationSpec,
        request: &AllocationRequest,
    ) -> Result<AllocationRequest> {
        self.transaction(|tx| {
            let old: Option<(String, String)> = tx
                .query_row(
                    "SELECT specification,request FROM fleet_allocation_specs WHERE id=?1",
                    [&spec.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((s, r)) = old {
                ensure!(
                    serde_json::from_str::<AllocationSpec>(&s)? == *spec,
                    "allocation retry identity mismatch"
                );
                return Ok(serde_json::from_str(&r)?);
            }
            tx.execute(
                "INSERT INTO fleet_allocation_specs VALUES(?1,?2,?3)",
                params![
                    spec.id,
                    serde_json::to_string(spec)?,
                    serde_json::to_string(request)?
                ],
            )?;
            Ok(request.clone())
        })
    }
    pub fn bound_spec(&self, spec: &AllocationSpec) -> Result<Option<AllocationRequest>> {
        self.db.with_connection(|db| {
            let old: Option<(String, String)> = db
                .query_row(
                    "SELECT specification,request FROM fleet_allocation_specs WHERE id=?1",
                    [&spec.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            old.map(|(s, r)| {
                ensure!(
                    serde_json::from_str::<AllocationSpec>(&s)? == *spec,
                    "allocation retry identity mismatch"
                );
                Ok(serde_json::from_str(&r)?)
            })
            .transpose()
        })
    }
    pub fn restore_bundle(&self, id: &str) -> Result<Option<EnvironmentBundle>> {
        self.db.with_connection(|db| {
            let json: Option<String> = db
                .query_row(
                    "SELECT specification FROM fleet_allocation_specs WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(json
                .map(|j| serde_json::from_str::<AllocationSpec>(&j))
                .transpose()?
                .and_then(|s| s.restore))
        })
    }
    pub fn feasibility(
        &self,
        request: &AllocationRequest,
        capacity: &Capacity,
        now: i64,
    ) -> Result<Feasibility> {
        request.validate().map_err(anyhow::Error::msg)?;
        capacity.validate(now).map_err(anyhow::Error::msg)?;
        self.db
            .with_connection(|db| feasible(request, capacity, &read_all(db)?))
    }
    /// Enqueue is idempotent and consumes no execution lease. Only admit grants capacity.
    pub fn enqueue(
        &self,
        request: AllocationRequest,
        capacity: &Capacity,
        now: i64,
    ) -> Result<Allocation> {
        request.validate().map_err(anyhow::Error::msg)?;
        capacity.validate(now).map_err(anyhow::Error::msg)?;
        self.transaction(|tx| {
            if let Some(old) = optional(tx, &request.id)? {
                ensure!(old.request == request, "allocation identity reused");
                return Ok(old);
            }
            let rows = read_all(tx)?;
            // The leader already performed spawn feasibility. Capacity may
            // change before enqueue; preserve accepted work without a lease.
            let _ = feasible(&request, capacity, &rows)?;
            let reason = None;
            let canceled: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM fleet_allocation_cancellations WHERE id=?1)",
                [&request.id],
                |r| r.get(0),
            )?;
            let order: i64 = tx.query_row(
                "SELECT COALESCE(MAX(served),0) FROM fleet_allocation_fairness",
                [],
                |r| r.get(0),
            )?;
            let participant = format!("{}:{}", request.leader, request.scan);
            for id in [
                participant.clone(),
                format!("{participant}:{}", request.branch),
            ] {
                tx.execute(
                    "INSERT OR IGNORE INTO fleet_allocation_fairness VALUES(?1,?2)",
                    params![id, order],
                )?;
            }
            let sequence: i64 = tx.query_row(
                "SELECT COALESCE(MAX(sequence),0)+1 FROM fleet_allocations",
                [],
                |r| r.get(0),
            )?;
            let row = Allocation {
                prepared: false,
                execution: None,
                request,
                phase: if canceled {
                    AllocationPhase::Canceled
                } else if reason.is_some() {
                    AllocationPhase::Blocked
                } else {
                    AllocationPhase::Queued
                },
                created_at_ms: now,
                reservation_deadline_ms: now.checked_add(300_000).context("clock overflow")?,
                reason,
                sequence: u64::try_from(sequence)?,
            };
            tx.execute(
                "INSERT INTO fleet_allocations VALUES(?1,?2,?3)",
                params![row.request.id, sequence, serde_json::to_string(&row)?],
            )?;
            Ok(row)
        })
    }
    /// The host chooses a fair candidate and rechecks current capacity in the
    /// same transaction that creates its reservation. Leaders cannot bypass it.
    pub fn admit_next(&self, capacity: &Capacity, now: i64) -> Result<Option<Allocation>> {
        capacity.validate(now).map_err(anyhow::Error::msg)?;
        self.transaction(|tx| {
            let rows=read_all(tx)?;
            let mut candidates=Vec::new();
            for row in &rows {
                if row.phase != AllocationPhase::Queued || row.request.domain != capacity.domain {continue;}
                let participant=format!("{}:{}",row.request.leader,row.request.scan);
                let branch=format!("{participant}:{}",row.request.branch);
                let served=|id:&str|->Result<i64>{Ok(tx.query_row("SELECT served FROM fleet_allocation_fairness WHERE participant=?1",[id],|r|r.get(0)).optional()?.unwrap_or(0))};
                candidates.push((served(&participant)?,served(&branch)?,!row.request.processing_result,row.sequence,row.clone(),participant,branch));
            }
            let order:i64=tx.query_row("SELECT COALESCE(MAX(served),0) FROM fleet_allocation_fairness",[],|r|r.get(0))?;
            candidates.sort_by_key(|c|(order%2==0 && c.2,c.0,c.1,c.3));
            for (_,_,_,_,mut row,participant,branch) in candidates {
                let lifecycle:Option<String>=tx.query_row("SELECT document FROM fleet_allocation_lifecycle WHERE leader=?1 AND scan=?2",params![row.request.leader,row.request.scan],|r|r.get(0)).optional()?;
                if let Some(lifecycle)=lifecycle && serde_json::from_str::<ScanLifecycle>(&lifecycle)?.ended_at_ms.is_some() {
                    row.phase=AllocationPhase::Blocked;row.reason=Some("Scan ended before admission; reopen it before requesting another allocation".into());write(tx,&row)?;continue;
                }
                if now > row.reservation_deadline_ms {
                    row.phase=AllocationPhase::Blocked;row.reason=Some("Allocation wait exhausted; capacity was not released. Stop lingering subagents with swarm_stop_subagent and retry.".into());write(tx,&row)?;continue;
                }
                if let Some(parent)=&row.request.release_parent {
                    let parent=rows.iter().find(|r| &r.request.id==parent).context("release dependency missing")?;
                    if matches!(parent.phase,AllocationPhase::Reserved|AllocationPhase::Started) {continue;}
                    if !matches!(parent.phase,AllocationPhase::Retained|AllocationPhase::Canceled) {continue;}
                    if parent.phase==AllocationPhase::Retained {
                        let protected=retained(tx,&parent.request.instance)?;
                        if !protected.checkpoint_confirmed || !protected.references.is_empty() {continue;}
                    }
                }
                if rows.iter().any(|other| active(other) && other.request.instance==row.request.instance) || !fits(&row.request,capacity,&rows,None)? {continue;}
                // A retained container cannot be admitted after cleanup wins.
                if let Some(mut retained)=retained_optional(tx,&row.request.instance)? {
                    if retained.deletion!=DeletionStep::Retained || retained.leader!=row.request.leader || retained.scan!=row.request.scan || retained.agent!=row.request.agent {
                        row.phase=AllocationPhase::Blocked;row.reason=Some("Retained environment is being deleted or belongs to another agent".into());write(tx,&row)?;continue;
                    }
                    if !retained.references.is_empty() || !retained.termination_confirmed || !retained.checkpoint_confirmed {continue;}
                    retained.references.insert(row.request.id.clone());
                    retained.ended_at_ms=None;retained.deadline_ms=None;
                    retained.generation=next(retained.generation)?;
                    save_retained(tx,&retained)?;
                } else {
                    save_retained(tx,&RetainedEnvironment {instance:row.request.instance.clone(),leader:row.request.leader.clone(),scan:row.request.scan.clone(),agent:row.request.agent.clone(),generation:1,ended_at_ms:None,deadline_ms:None,keep:false,references:BTreeSet::from([row.request.id.clone()]),pending_outputs:BTreeSet::new(),termination_confirmed:false,checkpoint_confirmed:false,deletion:DeletionStep::Retained})?;
                }
                row.phase=AllocationPhase::Reserved;
                row.reservation_deadline_ms=now.checked_add(300_000).context("clock overflow")?;
                write(tx,&row)?;
                let order:i64=tx.query_row("SELECT COALESCE(MAX(served),0)+1 FROM fleet_allocation_fairness",[],|r|r.get(0))?;
                for id in [participant,branch] {tx.execute("INSERT INTO fleet_allocation_fairness VALUES(?1,?2) ON CONFLICT(participant) DO UPDATE SET served=excluded.served",params![id,order])?;}
                return Ok(Some(row));
            }
            Ok(None)
        })
    }
    pub fn started(&self, id: &str) -> Result<()> {
        self.transaction(|tx| {
            let mut row = read(tx, id)?;
            ensure!(
                matches!(
                    row.phase,
                    AllocationPhase::Reserved | AllocationPhase::Started
                ),
                "allocation has no execution reservation"
            );
            row.phase = AllocationPhase::Started;
            let mut retained = retained(tx, &row.request.instance)?;
            ensure!(
                retained.deletion == DeletionStep::Retained && retained.references.contains(id),
                "environment not protected"
            );
            retained.termination_confirmed = false;
            retained.checkpoint_confirmed = false;
            retained.generation = next(retained.generation)?;
            save_retained(tx, &retained)?;
            write(tx, &row)
        })
    }
    pub fn bind_execution(
        &self,
        id: &str,
        target: &beale_contracts::swarm_host::ExecutionTarget,
    ) -> Result<()> {
        target.validate().map_err(anyhow::Error::msg)?;
        self.transaction(|tx| {
            let mut row = read(tx, id)?;
            ensure!(
                row.prepared
                    && matches!(
                        row.phase,
                        AllocationPhase::Reserved | AllocationPhase::Started
                    )
                    && row.request.leader == target.leader_id
                    && row.request.scan == target.identity.scan_id
                    && row.request.agent == target.identity.agent_id
                    && row.request.instance == target.instance_id
                    && row.request.configuration == target.configuration_id
                    && row.request.configuration_revision == target.configuration_revision,
                "execution differs from reservation"
            );
            ensure!(
                row.execution.as_ref().is_none_or(|old| old == target),
                "reservation already belongs to another execution"
            );
            row.execution = Some(target.clone());
            write(tx, &row)
        })
    }
    pub fn prepared(&self, id: &str) -> Result<()> {
        self.transaction(|tx| {
            let mut row = read(tx, id)?;
            ensure!(
                row.phase == AllocationPhase::Reserved,
                "allocation is not reserved"
            );
            row.prepared = true;
            write(tx, &row)
        })
    }
    /// A clock alone can never release a possibly running reservation. The
    /// backend must freshly establish stop (including never-created instances).
    pub fn confirm_stopped(&self, id: &str, checkpoint: bool) -> Result<()> {
        self.transaction(|tx| {
            let mut row = read(tx, id)?;
            ensure!(
                matches!(
                    row.phase,
                    AllocationPhase::Reserved
                        | AllocationPhase::Started
                        | AllocationPhase::Retained
                ),
                "allocation not owned"
            );
            let mut retained = retained(tx, &row.request.instance)?;
            // An acknowledged older execution must not change the lifecycle
            // of a newer reservation sharing this retained instance.
            if row.phase == AllocationPhase::Retained
                && !retained.references.contains(id)
                && read_all(tx)?.iter().any(|other| {
                    other.request.instance == row.request.instance
                        && other.sequence > row.sequence
                        && matches!(
                            other.phase,
                            AllocationPhase::Reserved
                                | AllocationPhase::Started
                                | AllocationPhase::Retained
                        )
                })
            {
                return Ok(());
            }
            ensure!(
                retained.deletion == DeletionStep::Retained,
                "environment deletion claimed"
            );
            retained.termination_confirmed = true;
            retained.checkpoint_confirmed |= checkpoint && retained.pending_outputs.is_empty();
            if checkpoint {
                retained.references.remove(id);
            }
            retained.generation = next(retained.generation)?;
            row.phase = AllocationPhase::Retained;
            write(tx, &row)?;
            save_retained(tx, &retained)
        })
    }
    pub fn cancel_queued(&self, id: &str) -> Result<()> {
        self.transaction(|tx| {
            ensure!(
                beale_contracts::swarms::valid_id(id),
                "invalid allocation identity"
            );
            tx.execute(
                "INSERT OR IGNORE INTO fleet_allocation_cancellations VALUES(?1)",
                [id],
            )?;
            let Some(mut row) = optional(tx, id)? else {
                return Ok(());
            };
            ensure!(
                matches!(
                    row.phase,
                    AllocationPhase::Queued | AllocationPhase::Canceled | AllocationPhase::Blocked
                ) || row.phase == AllocationPhase::Retained && row.execution.is_none(),
                "running allocation requires confirmed termination"
            );
            row.phase = AllocationPhase::Canceled;
            write(tx, &row)
        })
    }
    pub fn environment(&self, instance: &str) -> Result<RetainedEnvironment> {
        self.db.with_connection(|db| retained(db, instance))
    }
    pub fn preview_discard(
        &self,
        instance: &str,
        generation: u64,
        now: i64,
    ) -> Result<DiscardPreview> {
        self.transaction(|tx| {
            let row=retained(tx,instance)?;
            ensure!(row.generation==generation && row.deletion==DeletionStep::Retained && row.termination_confirmed && !row.pending_outputs.is_empty(),"discard requires an unchanged stopped environment with identified pending outputs");
            let allocations=read_all(tx)?;
            ensure!(row.references.iter().all(|id|allocations.iter().any(|a|a.request.id==*id && a.request.instance==instance && a.phase==AllocationPhase::Retained)),"environment is active or restoring");
            let preview=DiscardPreview{instance:instance.into(),generation,operations:row.pending_outputs,confirmation:uuid::Uuid::new_v4().to_string()};
            tx.execute("INSERT INTO fleet_retention_discards(confirmation,document,expires) VALUES(?1,?2,?3)",params![preview.confirmation,serde_json::to_string(&preview)?,now.checked_add(300_000).context("discard clock overflow")?])?;Ok(preview)
        })
    }
    pub fn confirm_discard(
        &self,
        preview: &DiscardPreview,
        now: i64,
    ) -> Result<RetainedEnvironment> {
        self.transaction(|tx| {
            let (json,expires,confirmed):(String,i64,bool)=tx.query_row("SELECT document,expires,confirmed FROM fleet_retention_discards WHERE confirmation=?1",[&preview.confirmation],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            ensure!(serde_json::from_str::<DiscardPreview>(&json)?==*preview,"discard confirmation differs from preview");
            let mut row=retained(tx,&preview.instance)?;
            if confirmed {return Ok(row);}
            ensure!(now<=expires && row.generation==preview.generation && row.pending_outputs==preview.operations && row.termination_confirmed && row.deletion==DeletionStep::Retained,"discard preview expired or environment changed");
            let allocations=read_all(tx)?;
            ensure!(row.references.iter().all(|id|allocations.iter().any(|a|a.request.id==*id && a.request.instance==row.instance && a.phase==AllocationPhase::Retained)),"environment is active or restoring");
            row.references.clear();row.pending_outputs.clear();row.checkpoint_confirmed=true;row.generation=next(row.generation)?;
            save_retained(tx,&row)?;tx.execute("UPDATE fleet_retention_discards SET confirmed=1 WHERE confirmation=?1",[&preview.confirmation])?;Ok(row)
        })
    }
    pub fn environments(&self) -> Result<Vec<RetainedEnvironment>> {
        self.db.with_connection(|db| {
            let mut q = db.prepare("SELECT document FROM fleet_retention ORDER BY instance")?;
            let docs = q
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<crate::rusqlite::Result<Vec<_>>>()?;
            docs.iter().map(|s| Ok(serde_json::from_str(s)?)).collect()
        })
    }
    pub fn retention(
        &self,
        instance: &str,
        generation: u64,
        action: RetentionAction,
        now: i64,
    ) -> Result<RetainedEnvironment> {
        ensure!(now >= 0, "invalid lifecycle clock");
        self.transaction(|tx| {
            let mut row=retained(tx,instance)?;
            ensure!(row.generation==generation,"stale retention generation");
            ensure!(row.deletion==DeletionStep::Retained,"environment deletion in progress or complete");
            let valid=|id:&str|->Result<()>{ensure!(beale_contracts::swarms::valid_id(id),"invalid protection identity");Ok(())};
            match action {
                RetentionAction::Keep{value}=>row.keep=value,
                RetentionAction::Protect{reference}=>{valid(&reference)?;row.references.insert(reference);},
                RetentionAction::Release{reference}=>{ensure!(!read_all(tx)?.iter().any(|a|a.request.id==reference && a.request.instance==instance && active(a)),"execution protection requires confirmed termination");ensure!(row.references.remove(&reference),"unknown environment protection");},
                RetentionAction::End{ended_at_ms,retention_seconds}=>{
                    ensure!((0..=now).contains(&ended_at_ms) && retention_seconds<=365*86_400,"invalid retention interval");
                    if row.ended_at_ms.is_none(){row.ended_at_ms=Some(ended_at_ms);row.deadline_ms=Some(ended_at_ms.checked_add(i64::try_from(retention_seconds)?.checked_mul(1000).context("retention overflow")?).context("deadline overflow")?);}
                },
                RetentionAction::Reopen=>{row.ended_at_ms=None;row.deadline_ms=None;},
                RetentionAction::Pending{operation}=>{valid(&operation)?;row.pending_outputs.insert(operation);row.checkpoint_confirmed=false;},
                RetentionAction::Acknowledge{operation}=>{ensure!(row.pending_outputs.remove(&operation),"unknown publication identity");},
                RetentionAction::Checkpoint=>{ensure!(row.termination_confirmed && row.pending_outputs.is_empty(),"checkpoint has pending execution or publications");row.checkpoint_confirmed=true;},
                RetentionAction::ClaimDelete{now:explicit}=>{
                    ensure!(row.references.is_empty(),"environment is active or restoring");
                    ensure!(row.termination_confirmed,"environment termination unconfirmed");
                    ensure!(row.pending_outputs.is_empty(),"environment has unacknowledged outputs; reconcile publication before deletion");
                    ensure!(row.checkpoint_confirmed,"final checkpoint is not durable");
                    ensure!(explicit || (!row.keep && row.ended_at_ms.is_some() && row.deadline_ms.is_some_and(|d|d<=now)),"environment retention is protected");
                    row.deletion=DeletionStep::Claimed;
                }
            }
            row.generation=next(row.generation)?;save_retained(tx,&row)?;Ok(row)
        })
    }
    /// Physical deletion journals advance only the exact claimed generation.
    pub fn deletion_step(&self, instance: &str, generation: u64, step: DeletionStep) -> Result<()> {
        self.transaction(|tx| {
            let mut row = retained(tx, instance)?;
            ensure!(row.generation == generation, "stale deletion generation");
            ensure!(
                row.deletion == step
                    || matches!(
                        (row.deletion, step),
                        (DeletionStep::Claimed, DeletionStep::ContainerRemoved)
                            | (DeletionStep::ContainerRemoved, DeletionStep::VolumeRemoved)
                            | (DeletionStep::VolumeRemoved, DeletionStep::Deleted)
                    ),
                "invalid physical deletion order"
            );
            row.deletion = step;
            if step == DeletionStep::Deleted {
                for mut allocation in read_all(tx)?.into_iter().filter(|a| {
                    a.request.instance == instance && a.phase == AllocationPhase::Retained
                }) {
                    allocation.phase = AllocationPhase::Canceled;
                    write(tx, &allocation)?;
                }
            }
            save_retained(tx, &row)
        })
    }
}
fn next(value: u64) -> Result<u64> {
    value.checked_add(1).context("generation exhausted")
}
fn read_all(db: &crate::rusqlite::Connection) -> Result<Vec<Allocation>> {
    let mut q = db.prepare("SELECT document FROM fleet_allocations ORDER BY sequence")?;
    let docs = q
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<crate::rusqlite::Result<Vec<_>>>()?;
    docs.iter().map(|s| Ok(serde_json::from_str(s)?)).collect()
}
fn optional(db: &crate::rusqlite::Connection, id: &str) -> Result<Option<Allocation>> {
    let s: Option<String> = db
        .query_row(
            "SELECT document FROM fleet_allocations WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    s.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
}
fn read(db: &crate::rusqlite::Connection, id: &str) -> Result<Allocation> {
    optional(db, id)?.context("unknown allocation")
}
fn write(tx: &Transaction<'_>, row: &Allocation) -> Result<()> {
    ensure!(
        tx.execute(
            "UPDATE fleet_allocations SET document=?2 WHERE id=?1",
            params![row.request.id, serde_json::to_string(row)?]
        )? == 1,
        "allocation disappeared"
    );
    Ok(())
}
fn retained_optional(
    db: &crate::rusqlite::Connection,
    id: &str,
) -> Result<Option<RetainedEnvironment>> {
    let s: Option<String> = db
        .query_row(
            "SELECT document FROM fleet_retention WHERE instance=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    s.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
}
fn retained(db: &crate::rusqlite::Connection, id: &str) -> Result<RetainedEnvironment> {
    retained_optional(db, id)?.context("unknown retained environment")
}
fn save_retained(tx: &Transaction<'_>, row: &RetainedEnvironment) -> Result<()> {
    tx.execute("INSERT INTO fleet_retention VALUES(?1,?2) ON CONFLICT(instance) DO UPDATE SET document=excluded.document",params![row.instance,serde_json::to_string(row)?])?;
    Ok(())
}
fn active(row: &Allocation) -> bool {
    matches!(
        row.phase,
        AllocationPhase::Reserved | AllocationPhase::Started
    )
}
fn fits(
    request: &AllocationRequest,
    capacity: &Capacity,
    rows: &[Allocation],
    release: Option<&Allocation>,
) -> Result<bool> {
    ensure!(
        request.domain == capacity.domain,
        "execution-domain measurement mismatch"
    );
    let mut memory = u128::from(capacity.available_memory);
    if let Some(parent) = release {
        memory += u128::from(
            *capacity
                .measured_memory
                .get(&parent.request.id)
                .unwrap_or(&0),
        );
    }
    let mut charged = u128::from(request.memory) + u128::from(RAM_FLOOR);
    let mut storage = BTreeMap::<(&str, &str), u64>::new();
    let mut reserve = request.storage_reserve;
    for row in rows.iter().filter(|r| {
        r.request.id != request.id
            && !matches!(
                r.phase,
                AllocationPhase::Queued | AllocationPhase::Blocked | AllocationPhase::Canceled
            )
    }) {
        reserve = reserve.max(row.request.storage_reserve);
        if row.request.domain == request.domain
            && active(row)
            && release.is_none_or(|p| p.request.id != row.request.id)
        {
            charged += u128::from(
                row.request
                    .memory
                    .saturating_sub(*capacity.measured_memory.get(&row.request.id).unwrap_or(&0)),
            );
        }
        // Suspending execution does not make retained filesystem bytes free.
        for demand in &row.request.storage {
            if capacity.storage.contains_key(&demand.pool)
                && !capacity.measured_storage.contains(&demand.key)
            {
                storage
                    .entry((&demand.pool, &demand.key))
                    .and_modify(|n| *n = (*n).max(demand.bytes))
                    .or_insert(demand.bytes);
            }
        }
    }
    if memory <= charged {
        return Ok(false);
    }
    for demand in &request.storage {
        if !capacity.measured_storage.contains(&demand.key) {
            storage
                .entry((&demand.pool, &demand.key))
                .and_modify(|n| *n = (*n).max(demand.bytes))
                .or_insert(demand.bytes);
        }
    }
    let mut totals = BTreeMap::<&str, u128>::new();
    // Even fully measured retained data must leave the reserve intact.
    for demand in &request.storage {
        totals.entry(&demand.pool).or_default();
    }
    for ((pool, _), bytes) in storage {
        *totals.entry(pool).or_default() += u128::from(bytes);
    }
    for (pool, bytes) in totals {
        let available = capacity
            .storage
            .get(pool)
            .context("Storage capacity is unknown; inspect the destination backing filesystem")?;
        if u128::from(*available) < bytes + u128::from(reserve) {
            return Ok(false);
        }
    }
    Ok(true)
}
fn feasible(
    request: &AllocationRequest,
    capacity: &Capacity,
    rows: &[Allocation],
) -> Result<Feasibility> {
    if let Some(id) = &request.release_parent {
        let parent = rows
            .iter()
            .find(|r| r.request.id == *id)
            .context("identified parent release is unavailable")?;
        ensure!(
            parent.request.leader == request.leader
                && parent.request.scan == request.scan
                && parent.request.agent == request.branch
                && parent.request.domain == request.domain,
            "unrelated parent release"
        );
    }
    if fits(request, capacity, rows, None)? {
        return Ok(Feasibility::Ready);
    }
    if let Some(id) = &request.release_parent {
        let parent = rows
            .iter()
            .find(|r| r.request.id == *id)
            .context("identified parent release is unavailable")?;
        ensure!(
            parent.request.leader == request.leader
                && parent.request.scan == request.scan
                && parent.request.agent == request.branch
                && parent.request.domain == request.domain,
            "unrelated parent release"
        );
        if active(parent) && fits(request, capacity, rows, Some(parent))? {
            return Ok(Feasibility::ReleaseParent {
                allocation: id.clone(),
            });
        }
    }
    Ok(Feasibility::Denied{reason:"Insufficient RAM or storage after reservations and the requesting parent's safe release; stop lingering subagents using swarm_stop_subagent or add capacity.".into()})
}
