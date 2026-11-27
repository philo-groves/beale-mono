//! Short SQLite transactions only: admission never waits while owning a lock.
use crate::{HostDatabase, fleet_executions::FleetExecutionStore};
use anyhow::{Context, Result, ensure};
use beale_contracts::{
    pacing::*,
    swarms::{PacingAmount, PacingTarget, valid_id},
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

#[derive(Clone)]
pub struct AccountingStore {
    db: HostDatabase,
}
pub fn bounded_snapshot(mut state: AccountingSnapshot) -> AccountingSnapshot {
    let omitted = state.reservations.len().saturating_sub(128);
    state.reservations.drain(..omitted);
    state.omitted_reservations = state.omitted_reservations.saturating_add(omitted as u64);
    state
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct OutboxEntry {
    pub request: Admission,
    pub allocation: Option<PacingTarget>,
    pub charge: Option<Charge>,
    pub canceled: bool,
    pub local_admitted: bool,
    pub dispatched: bool,
    pub local_estimate: u64,
    pub observed_charge: Option<Charge>,
}

pub fn amount(target: &PacingTarget) -> u64 {
    match &target.amount {
        PacingAmount::Subscription { basis_points, .. } => u64::from(*basis_points),
        PacingAmount::Currency { micros, .. } | PacingAmount::Credits { micros } => *micros,
    }
}
fn unit(target: &PacingTarget) -> AccountingUnit {
    match &target.amount {
        PacingAmount::Subscription { window_id, .. } => AccountingUnit::Subscription {
            window_id: window_id.clone(),
        },
        PacingAmount::Currency { currency, .. } => AccountingUnit::Currency {
            currency: currency.clone(),
        },
        PacingAmount::Credits { .. } => AccountingUnit::Credits,
    }
}

fn validate_snapshot_unit(state: &AccountingSnapshot) -> Result<()> {
    match &state.unit {
        Some(AccountingUnit::Subscription { window_id }) => {
            ensure!(valid_id(window_id), "invalid accounting window")
        }
        Some(AccountingUnit::Currency { currency }) => ensure!(
            currency.len() == 3 && currency.bytes().all(|b| b.is_ascii_uppercase()),
            "invalid accounting currency"
        ),
        _ => (),
    }
    for reservation in state.reservations.iter().filter(|r| !r.canceled) {
        for target in reservation
            .request
            .target
            .iter()
            .chain(reservation.aggregate_target.iter())
        {
            ensure!(
                state.unit.as_ref() == Some(&unit(target)),
                "handover accounting unit conflicts with retained history"
            );
        }
    }
    Ok(())
}
pub fn conservative_allocation(
    allocation: Option<PacingTarget>,
    target: Option<&PacingTarget>,
) -> Result<Option<PacingTarget>> {
    match (allocation, target) {
        (Some(a), Some(t)) => {
            ensure!(
                unit(&a) == unit(t),
                "accounting unit changed; configure a new account group"
            );
            Ok(Some(
                if u128::from(amount(&a)) * u128::from(t.interval_seconds)
                    > u128::from(amount(t)) * u128::from(a.interval_seconds)
                {
                    t.clone()
                } else {
                    a
                },
            ))
        }
        (allocation, _) => Ok(allocation),
    }
}
pub fn validate_target(target: &PacingTarget) -> Result<()> {
    ensure!(
        valid_id(&target.budget_group_id)
            && (1..=604800).contains(&target.interval_seconds)
            && amount(target) > 0
            && amount(target) <= 1_000_000_000_000,
        "invalid pacing target"
    );
    match &target.amount {
        PacingAmount::Subscription {
            window_id,
            basis_points,
        } => ensure!(
            valid_id(window_id) && *basis_points <= 10000,
            "invalid allowance window"
        ),
        PacingAmount::Currency { currency, .. } => ensure!(
            currency.len() == 3 && currency.bytes().all(|b| b.is_ascii_uppercase()),
            "invalid currency"
        ),
        _ => (),
    }
    Ok(())
}
fn aggregate_target(state: &AccountingSnapshot, now: u64) -> Option<PacingTarget> {
    state
        .reservations
        .iter()
        .filter(|r| !r.canceled && now.saturating_sub(r.touched_at_ms) <= 60_000)
        .filter_map(|r| r.request.target.as_ref())
        .max_by(|a, b| {
            (u128::from(amount(a)) * u128::from(b.interval_seconds))
                .cmp(&(u128::from(amount(b)) * u128::from(a.interval_seconds)))
        })
        .cloned()
}

fn validate_group_units(state: &AccountingSnapshot, target: Option<&PacingTarget>) -> Result<()> {
    if let Some(target) = target {
        ensure!(
            state
                .unit
                .as_ref()
                .is_none_or(|known| *known == unit(target)),
            "incompatible accounting units"
        );
        // Canceled, undispatched tickets carry no accounting units. In
        // particular, cleanup of a rejected request must not poison the group.
        ensure!(
            state.reservations.iter().filter(|r| !r.canceled).all(|r| r
                .request
                .target
                .iter()
                .chain(r.aggregate_target.iter())
                .all(|old| unit(old) == unit(target))),
            "incompatible accounting units"
        );
    }
    Ok(())
}

fn settle_local(
    tx: &rusqlite::Transaction<'_>,
    entry: &OutboxEntry,
    cost: Option<u64>,
) -> Result<()> {
    if !entry.local_admitted {
        return Ok(());
    }
    let Some(cost) = cost else {
        return Ok(());
    };
    let Some(target) = entry.allocation.as_ref().or(entry.request.target.as_ref()) else {
        return Ok(());
    };
    let debt = duration(target, cost.saturating_sub(entry.local_estimate));
    if tx.execute(
        "INSERT OR IGNORE INTO provider_accounting_local_charges VALUES(?1)",
        [&entry.request.id],
    )? == 1
    {
        let old: i64 = tx.query_row(
            "SELECT next_at_ms FROM provider_accounting_local WHERE group_id=?1",
            [&entry.request.group_id],
            |r| r.get(0),
        )?;
        let next = (old.max(0) as u64)
            .saturating_add(debt)
            .min(i64::MAX as u64) as i64;
        tx.execute(
            "UPDATE provider_accounting_local SET next_at_ms=?1,last_cost=?2 WHERE group_id=?3",
            params![next, i64::try_from(cost)?, entry.request.group_id],
        )?;
    }
    Ok(())
}

fn duration(target: &PacingTarget, cost: u64) -> u64 {
    ((u128::from(cost) * u128::from(target.interval_seconds) * 1000)
        .div_ceil(u128::from(amount(target))))
    .min(u128::from(u64::MAX)) as u64
}
pub fn divided(mut target: PacingTarget, count: u64) -> PacingTarget {
    target.interval_seconds = target
        .interval_seconds
        .saturating_mul(u32::try_from(count.max(1)).unwrap_or(u32::MAX));
    target
}

impl AccountingStore {
    pub fn open(db: HostDatabase) -> Result<Self> {
        db.migrate_schema("provider-accounting", &["CREATE TABLE provider_accounting_groups(id TEXT PRIMARY KEY, document TEXT NOT NULL);
          CREATE TABLE provider_accounting_routes(route TEXT PRIMARY KEY, group_id TEXT NOT NULL, explicit INTEGER NOT NULL DEFAULT 1);
          CREATE TABLE provider_accounting_outbox(id TEXT PRIMARY KEY, document TEXT NOT NULL);
          CREATE TABLE provider_accounting_local(group_id TEXT PRIMARY KEY, next_at_ms INTEGER NOT NULL, last_cost INTEGER NOT NULL);
          CREATE TABLE provider_accounting_local_charges(id TEXT PRIMARY KEY);
          CREATE TABLE provider_accounting_warnings(scan_id TEXT NOT NULL, warning TEXT NOT NULL, PRIMARY KEY(scan_id,warning));
          CREATE TABLE provider_accounting_allocations(group_id TEXT PRIMARY KEY, document TEXT NOT NULL);
          CREATE TABLE provider_accounting_probes(group_id TEXT PRIMARY KEY, at_ms INTEGER NOT NULL);"])?;
        Ok(Self { db })
    }
    pub fn host_id(&self) -> Result<String> {
        FleetExecutionStore::open(self.db.clone())?.host_id()
    }
    pub fn coordination_mode(&self, group: &str, mode: CoordinationMode) -> Result<()> {
        self.update(group, |state| {
            state.coordination_mode = Some(mode);
            Ok(())
        })
    }
    pub fn scan_for_session(&self, session: &str) -> Result<Option<String>> {
        self.db.with_connection(|db| {
            let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='swarm_scans')",[],|r|r.get(0))?;
            if !exists {return Ok(None)}
            Ok(db.query_row("SELECT id FROM swarm_scans WHERE id=?1 OR EXISTS(SELECT 1 FROM json_each(document,'$.agents') a WHERE json_extract(a.value,'$.summary.id')=?1) LIMIT 1",[session],|r|r.get(0)).optional()?)
        })
    }
    pub fn recover_abandoned(&self) -> Result<()> {
        for mut entry in self.outbox()? {
            if entry.charge.is_none() && !entry.canceled {
                if entry.dispatched {
                    entry.charge = Some(entry.observed_charge.clone().unwrap_or(Charge {
                        attempt_id: entry.request.id.clone(),
                        amount: None,
                        quality: CostQuality::Unknown,
                        uncertain: true,
                        input_tokens: None,
                        output_tokens: None,
                        cache_read_tokens: None,
                    }));
                    if let Some(charge) = &mut entry.charge {
                        charge.uncertain = true;
                    }
                    self.warn(&entry.request.scan_id,"A provider attempt was interrupted by host restart; its charge is uncertain.")?;
                } else {
                    entry.canceled = true;
                }
            }
            // Includes charges saved before a prior settlement/write failure.
            self.enqueue(&entry)?;
        }
        Ok(())
    }
    pub fn binding(&self, route: &str) -> Result<Option<AccountBinding>> {
        let id = self.db.with_connection(|db| {
            Ok(db
                .query_row(
                    "SELECT group_id FROM provider_accounting_routes WHERE route=?1",
                    [route],
                    |r| r.get::<_, String>(0),
                )
                .optional()?)
        })?;
        id.map(|id| {
            self.snapshot(&id)?
                .binding
                .context("missing accounting binding")
        })
        .transpose()
    }
    pub fn explicit_binding(&self, route: &str) -> Result<Option<AccountBinding>> {
        let explicit = self.db.with_connection(|db| {
            Ok(db
                .query_row(
                    "SELECT explicit FROM provider_accounting_routes WHERE route=?1",
                    [route],
                    |r| r.get::<_, bool>(0),
                )
                .optional()?
                .unwrap_or(false))
        })?;
        if explicit {
            self.binding(route)
        } else {
            Ok(None)
        }
    }
    pub fn bind_automatic(&self, route: &str, binding: AccountBinding) -> Result<()> {
        self.bind(route, binding)?;
        self.db.with_connection(|db| {
            db.execute(
                "UPDATE provider_accounting_routes SET explicit=0 WHERE route=?1",
                [route],
            )?;
            Ok(())
        })
    }
    pub fn snapshot(&self, id: &str) -> Result<AccountingSnapshot> {
        self.db.with_connection(|db| {
            Ok(serde_json::from_str(&db.query_row(
                "SELECT document FROM provider_accounting_groups WHERE id=?1",
                [id],
                |r| r.get::<_, String>(0),
            )?)?)
        })
    }
    fn update<T>(
        &self,
        id: &str,
        f: impl FnOnce(&mut AccountingSnapshot) -> Result<T>,
    ) -> Result<T> {
        self.db.with_connection(|db|{
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let text=tx.query_row("SELECT document FROM provider_accounting_groups WHERE id=?1",[id],|r|r.get::<_,String>(0)).optional()?;
            let mut state=text.map(|v|serde_json::from_str(&v)).transpose()?.unwrap_or_default();
            let result=f(&mut state)?;
            tx.execute("INSERT INTO provider_accounting_groups VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET document=excluded.document",params![id,serde_json::to_string(&state)?])?;
            tx.commit()?;Ok(result)
        })
    }
    pub fn bind(&self, route: &str, binding: AccountBinding) -> Result<()> {
        ensure!(
            !route.is_empty()
                && route.len() <= 512
                && valid_id(&binding.group_id)
                && valid_id(&binding.provider_id)
                && valid_id(&binding.owner_host_id)
                && binding.generation > 0
                && !binding.participants.is_empty()
                && binding.participants.len() <= 256
                && binding.participants.iter().all(|p| valid_id(p)),
            "invalid account binding"
        );
        let mut unique = binding.participants.clone();
        unique.sort();
        unique.dedup();
        ensure!(
            unique.len() == binding.participants.len(),
            "duplicate accounting participant"
        );
        self.update(&binding.group_id, |state| {
            if let Some(old) = &state.binding {
                ensure!(
                    old == &binding,
                    "account binding changes require reconciled handover"
                );
            }
            state.binding = Some(binding.clone());
            Ok(())
        })?;
        self.db.with_connection(|db|{db.execute("INSERT INTO provider_accounting_routes VALUES(?1,?2,1) ON CONFLICT(route) DO UPDATE SET group_id=excluded.group_id,explicit=1",params![route,binding.group_id])?;Ok(())})
    }
    pub fn freeze(&self, id: &str, generation: u64) -> Result<AccountingSnapshot> {
        let host = self.host_id()?;
        self.update(id, |s| {
            let b = s.binding.as_ref().context("unknown group")?;
            ensure!(
                b.owner_host_id == host && b.generation == generation,
                "stale accounting owner"
            );
            ensure!(
                b.participants
                    .iter()
                    .all(|p| s.drained_participants.contains(p)),
                "all participants must drain before handover"
            );
            ensure!(
                s.reservations
                    .iter()
                    .all(|r| r.canceled || r.charge.is_some()),
                "reconcile outstanding reservations before handover"
            );
            s.frozen = true;
            Ok(bounded_snapshot(s.clone()))
        })
    }
    pub fn drain(&self, id: &str, generation: u64, participant: &str) -> Result<()> {
        self.update(id, |s| {
            let b = s.binding.as_ref().context("unknown group")?;
            ensure!(
                b.generation == generation && b.participants.iter().any(|p| p == participant),
                "stale accounting participant"
            );
            ensure!(
                s.reservations
                    .iter()
                    .filter(|r| r.request.participant_id == participant)
                    .all(|r| r.canceled || r.charge.is_some()),
                "participant has outstanding reservations"
            );
            if !s.drained_participants.iter().any(|p| p == participant) {
                s.drained_participants.push(participant.into());
            }
            Ok(())
        })
    }
    pub fn import(&self, mut snapshot: AccountingSnapshot, owner: &str) -> Result<()> {
        let host = self.host_id()?;
        ensure!(
            snapshot.frozen && valid_id(owner),
            "handover requires a frozen snapshot"
        );
        validate_snapshot_unit(&snapshot)?;
        ensure!(
            snapshot
                .reservations
                .iter()
                .all(|r| r.canceled || r.charge.is_some()),
            "unreconciled handover"
        );
        let binding = snapshot.binding.as_mut().context("missing binding")?;
        ensure!(
            binding
                .participants
                .iter()
                .all(|p| snapshot.drained_participants.contains(p)),
            "unreconciled participants"
        );
        ensure!(
            valid_id(&binding.group_id)
                && valid_id(&binding.provider_id)
                && valid_id(&binding.owner_host_id)
                && binding.generation > 0
                && !binding.participants.is_empty()
                && binding.participants.len() <= 256
                && binding.participants.iter().all(|p| valid_id(p)),
            "invalid handover binding"
        );
        let mut ids = std::collections::HashSet::new();
        for reservation in &snapshot.reservations {
            let r = &reservation.request;
            ensure!(
                valid_id(&r.id)
                    && ids.insert(&r.id)
                    && valid_id(&r.scan_id)
                    && r.group_id == binding.group_id
                    && r.generation > 0
                    && r.generation <= binding.generation
                    && binding.participants.contains(&r.participant_id),
                "invalid handover reservation"
            );
            if let Some(t) = &reservation.aggregate_target {
                validate_target(t)?;
                ensure!(
                    t.budget_group_id == binding.group_id,
                    "invalid aggregate target"
                );
                ensure!(
                    r.target.as_ref().is_none_or(|own| unit(own) == unit(t)),
                    "incompatible aggregate units"
                );
            }
            if let Some(t) = &r.target {
                validate_target(t)?;
                ensure!(
                    t.budget_group_id == binding.group_id,
                    "invalid handover target"
                );
            }
            if let Some(c) = &reservation.charge {
                ensure!(
                    c.attempt_id == r.id
                        && c.amount.is_some() == (c.quality != CostQuality::Unknown),
                    "invalid handover charge"
                );
            }
        }
        ensure!(
            snapshot.observations.len() <= 64
                && snapshot.observations.iter().all(|o| valid_id(&o.window_id)
                    && o.used_basis_points <= 10000
                    && o.resets_at_ms > o.observed_at_ms),
            "invalid handover observation"
        );
        let id = binding.group_id.clone();
        let previous = binding.generation;
        binding.generation = binding
            .generation
            .checked_add(1)
            .context("generation exhausted")?;
        binding.owner_host_id = owner.into();
        snapshot.frozen = false;
        snapshot.drained_participants.clear();
        self.update(&id, |old| {
            if let Some(b) = &old.binding {
                ensure!(
                    b.generation == previous && (old.frozen || b.owner_host_id != host),
                    "stale or active authority"
                );
            }
            ensure!(
                old.unit
                    .as_ref()
                    .is_none_or(|known| snapshot.unit.as_ref() == Some(known)),
                "handover cannot change established accounting units"
            );
            // Prices belong to this inference host. A transfer must not replace
            // its configuration with another host's model rates.
            snapshot.prices = std::mem::take(&mut old.prices);
            // Keep locally held history omitted from the bounded transfer. The
            // incoming generation owns scheduling state, not historical deletion.
            let incoming: std::collections::HashSet<_> = snapshot
                .reservations
                .iter()
                .map(|r| r.request.id.clone())
                .collect();
            let mut retained: Vec<_> = old
                .reservations
                .iter()
                .filter(|r| !incoming.contains(&r.request.id))
                .cloned()
                .collect();
            ensure!(
                retained.iter().all(|r| r.canceled || r.charge.is_some()),
                "local accounting is not drained"
            );
            snapshot.omitted_reservations = snapshot
                .omitted_reservations
                .saturating_sub(retained.len() as u64);
            retained.append(&mut snapshot.reservations);
            snapshot.reservations = retained;
            validate_snapshot_unit(&snapshot)?;
            *old = snapshot;
            Ok(())
        })
    }
    fn validate_request(state: &AccountingSnapshot, r: &Admission, host: &str) -> Result<()> {
        let b = state.binding.as_ref().context("unknown accounting group")?;
        ensure!(
            !state.frozen
                && b.owner_host_id == host
                && b.generation == r.generation
                && b.group_id == r.group_id,
            "stale accounting authority"
        );
        ensure!(
            valid_id(&r.id) && valid_id(&r.scan_id) && b.participants.contains(&r.participant_id),
            "invalid accounting participant"
        );
        if let Some(t) = &r.target {
            validate_target(t)?;
            ensure!(t.budget_group_id == r.group_id, "wrong budget group");
        }
        Ok(())
    }
    pub fn admit(&self, r: &Admission, now: u64) -> Result<AdmissionReply> {
        let host = self.host_id()?;
        self.update(&r.group_id,|s|{
            Self::validate_request(s,r,&host)?;
            s.coordination_mode=Some(CoordinationMode::Coordinated);
            ensure!(!s.drained_participants.contains(&r.participant_id),"accounting participant is drained for handover");
            validate_group_units(s,r.target.as_ref())?;
            if let Some(existing)=s.reservations.iter_mut().find(|p|p.request.id==r.id){
                ensure!(existing.request==*r && !existing.canceled,"changed or canceled admission");existing.touched_at_ms=now;
            } else {
                s.reservations.push(Reservation{request:r.clone(),queued_at_ms:now,touched_at_ms:now,admitted:false,canceled:false,estimate:0,aggregate_target:None,charge:None});
            }
            // A validated queued request also establishes denomination, even
            // if its first call must wait for an exhausted window to reset.
            if s.unit.is_none() {s.unit=r.target.as_ref().map(unit);}
            // Expire abandoned *waiting* tickets, never interrupt in-flight inference.
            for p in &mut s.reservations {if !p.admitted && now.saturating_sub(p.touched_at_ms)>60000 {p.canceled=true;}}
            let active_scans=s.reservations.iter().filter(|p|!p.canceled&&now.saturating_sub(p.touched_at_ms)<=60000).map(|p|(&p.request.participant_id,&p.request.scan_id)).collect::<std::collections::HashSet<_>>().len();
            // Aggregate ceiling is independent of each scan's own ceiling.
            let aggregate=aggregate_target(s,now);
            let allocation=aggregate.clone().map(|t|divided(t,active_scans as u64));
            let key=format!("{}:{}",r.participant_id,r.scan_id);
            let mut at=s.next_at_ms.max(*s.scan_next.get(&key).unwrap_or(&0));
            let mut warnings=vec!["Provider pacing is best effort, not a spending cap.".into()];
            if s.binding.as_ref().is_some_and(|b|b.limited_identity){warnings.push("Account identity is local; cross-host accounting is unavailable until explicitly grouped.".into());}
            // Account denomination survives rate expiry and bounded handover.
            let allowance_unit=s.unit.clone().or_else(||r.target.as_ref().or(aggregate.as_ref()).map(unit));
            if let Some(AccountingUnit::Subscription{window_id})=allowance_unit {
                if let Some(o)=s.observations.iter().find(|o|o.window_id==window_id && o.resets_at_ms>now && now.saturating_sub(o.observed_at_ms)<=60000){
                    if o.used_basis_points>=10000 {at=at.max(o.resets_at_ms);}
                } else {warnings.push("Allowance measurements are stale or unavailable; usage is uncertain.".into());}
            }
            let index=s.reservations.iter().position(|p|p.request.id==r.id).context("missing admission")?;
            if !s.reservations[index].admitted {
                let waiting=|p:&&Reservation| !p.admitted && !p.canceled && p.charge.is_none()
                    && *s.scan_next.get(&format!("{}:{}",p.request.participant_id,p.request.scan_id)).unwrap_or(&0)<=now;
                let next_class=match s.last_class {Some(CallClass::Research)=>CallClass::Supporting,_=>CallClass::Research};
                let chosen=s.reservations.iter().filter(waiting).filter(|p|p.request.class==next_class).min_by_key(|p|p.queued_at_ms)
                    .or_else(||s.reservations.iter().filter(waiting).min_by_key(|p|p.queued_at_ms)).map(|p|p.request.id.clone());
                if at<=now && chosen.as_deref()==Some(&r.id) {
                    if s.unit.is_none() {s.unit=r.target.as_ref().or(aggregate.as_ref()).map(unit);}
                    let estimate=s.reservations.iter().rev().filter_map(|p|p.charge.as_ref().and_then(|c|c.amount)).find(|n|*n>0)
                        .unwrap_or_else(||r.target.as_ref().or(aggregate.as_ref()).map_or(0,|t|(amount(t)/60).max(1)));
                    s.reservations[index].estimate=estimate;s.reservations[index].aggregate_target=aggregate.clone();s.reservations[index].admitted=true;s.last_class=Some(r.class.clone());
                    if let Some(t)=&aggregate {s.next_at_ms=now.saturating_add(duration(t,estimate));}
                    if let Some(t)=&r.target {s.scan_next.insert(key,now.saturating_add(duration(t,estimate)));}
                }
            }
            Ok(AdmissionReply{admitted:s.reservations[index].admitted,retry_at_ms:at.max(now.saturating_add(250)),allocation,warnings})
        })
    }
    pub fn settle(&self, r: &Admission, charge: &Charge) -> Result<()> {
        let host = self.host_id()?;
        self.update(&r.group_id, |s| {
            Self::validate_request(s, r, &host)?;
            ensure!(
                charge.attempt_id == r.id
                    && (charge.amount.is_some() == (charge.quality != CostQuality::Unknown)),
                "invalid usage event"
            );
            let p = s
                .reservations
                .iter_mut()
                .find(|p| p.request.id == r.id)
                .context("unknown reservation")?;
            ensure!(
                p.request == *r && p.admitted && !p.canceled,
                "unadmitted charge"
            );
            if let Some(old) = &p.charge {
                ensure!(old == charge, "conflicting usage redelivery");
                return Ok(());
            }
            let effective_cost = charge.amount.unwrap_or_else(|| {
                p.estimate.max(
                    r.target
                        .as_ref()
                        .or(p.aggregate_target.as_ref())
                        .map_or(0, |t| (amount(t) / 60).max(1)),
                )
            });
            let delta = effective_cost.saturating_sub(p.estimate);
            if let Some(t) = &p.aggregate_target {
                s.next_at_ms = s.next_at_ms.saturating_add(duration(t, delta));
            }
            if let Some(t) = &r.target {
                let n = s
                    .scan_next
                    .entry(format!("{}:{}", r.participant_id, r.scan_id))
                    .or_default();
                *n = n.saturating_add(duration(t, delta));
            }
            p.charge = Some(charge.clone());
            Ok(())
        })
    }
    /// Reconcile a disconnected physical attempt without dispatching it again.
    pub fn reconcile(&self, r: &Admission, charge: &Charge, now: u64) -> Result<()> {
        ensure!(
            charge.attempt_id == r.id
                && charge.amount.is_some() == (charge.quality != CostQuality::Unknown),
            "invalid usage event"
        );
        let host = self.host_id()?;
        self.update(&r.group_id, |s| {
            Self::validate_request(s, r, &host)?;
            validate_group_units(s, r.target.as_ref())?;
            let needs_rate = s
                .reservations
                .iter()
                .find(|p| p.request.id == r.id)
                .is_none_or(|p| !p.admitted);
            if let Some(p) = s.reservations.iter_mut().find(|p| p.request.id == r.id) {
                ensure!(p.request == *r, "changed admission");
                if !p.admitted {
                    s.next_at_ms = s.next_at_ms.max(now);
                    p.admitted = true;
                    p.canceled = false;
                    p.touched_at_ms = now;
                }
            } else {
                s.next_at_ms = s.next_at_ms.max(now);
                s.reservations.push(Reservation {
                    request: r.clone(),
                    queued_at_ms: now,
                    touched_at_ms: now,
                    admitted: true,
                    canceled: false,
                    estimate: 0,
                    aggregate_target: None,
                    charge: None,
                });
            }
            if needs_rate {
                // An offline attempt has no authoritative scan reservation.
                // Anchor both debts at reconciliation time, preserving future
                // debt and leaving already-granted/replayed attempts unchanged.
                if r.target.is_some() {
                    let deadline = s
                        .scan_next
                        .entry(format!("{}:{}", r.participant_id, r.scan_id))
                        .or_default();
                    *deadline = (*deadline).max(now);
                }
                let aggregate = aggregate_target(s, now);
                if s.unit.is_none() {
                    s.unit = r.target.as_ref().or(aggregate.as_ref()).map(unit);
                }
                s.reservations
                    .iter_mut()
                    .find(|p| p.request.id == r.id)
                    .context("missing reconciled reservation")?
                    .aggregate_target = aggregate;
            }
            Ok(())
        })?;
        self.settle(r, charge)
    }
    pub fn cancel(&self, r: &Admission) -> Result<()> {
        let host = self.host_id()?;
        self.update(&r.group_id, |s| {
            Self::validate_request(s, r, &host)?;
            if let Some(p) = s.reservations.iter_mut().find(|p| p.request.id == r.id) {
                ensure!(p.request == *r, "changed admission");
                if !p.admitted {
                    p.canceled = true;
                } else if p.charge.is_none() {
                    // The grant may have been lost before the participant could
                    // record dispatch. Keep its reserved debt as uncertain.
                    p.charge = Some(Charge {
                        attempt_id: r.id.clone(),
                        amount: None,
                        quality: CostQuality::Unknown,
                        uncertain: true,
                        input_tokens: None,
                        output_tokens: None,
                        cache_read_tokens: None,
                    });
                }
            } else {
                // A delayed admission must not resurrect an acknowledged cancel.
                s.reservations.push(Reservation {
                    request: r.clone(),
                    queued_at_ms: 0,
                    touched_at_ms: 0,
                    admitted: false,
                    canceled: true,
                    estimate: 0,
                    aggregate_target: None,
                    charge: None,
                });
            }
            Ok(())
        })
    }
    pub fn observe(&self, id: &str, generation: u64, o: Observation, now: u64) -> Result<()> {
        ensure!(
            valid_id(&o.window_id)
                && o.used_basis_points <= 10000
                && o.observed_at_ms <= now
                && o.resets_at_ms > o.observed_at_ms,
            "invalid allowance observation"
        );
        let host = self.host_id()?;
        self.update(id,|s|{
            let b=s.binding.as_ref().context("unknown group")?;ensure!(!s.frozen && b.owner_host_id==host && b.generation==generation,"stale accounting authority");
            if let Some(old)=s.observations.iter().find(|v|v.window_id==o.window_id){ensure!(o.observed_at_ms>=old.observed_at_ms,"stale observation");
                if old.resets_at_ms==o.resets_at_ms {
                    let reserved=s.reservations.iter().filter(|r|r.admitted && r.touched_at_ms>=old.observed_at_ms && r.touched_at_ms<o.observed_at_ms).map(|r|r.estimate).sum::<u64>();
                    let delta=u64::from(o.used_basis_points.saturating_sub(old.used_basis_points)).saturating_sub(reserved);
                    if let Some(t)=aggregate_target(s,now).filter(|t|matches!(&t.amount,PacingAmount::Subscription{window_id,..} if window_id==&o.window_id)) {
                        s.next_at_ms=s.next_at_ms.max(now).saturating_add(duration(&t,delta));
                    }
                }
            }
            s.observations.retain(|v|v.window_id!=o.window_id);s.observations.push(o);Ok(())
        })
    }
    pub fn price(&self, id: &str, price: Price) -> Result<()> {
        ensure!(
            !price.model.is_empty()
                && price.model.len() <= 512
                && (price.currency_or_credit == "credits"
                    || (price.currency_or_credit.len() == 3
                        && price
                            .currency_or_credit
                            .bytes()
                            .all(|b| b.is_ascii_uppercase()))),
            "invalid pricing"
        );
        self.update(id, |s| {
            ensure!(s.binding.is_some(), "unknown group");
            s.prices.retain(|p| p.model != price.model);
            s.prices.push(price);
            Ok(())
        })
    }
    pub fn warn(&self, scan: &str, warning: &str) -> Result<()> {
        self.db.with_connection(|db| {
            db.execute(
                "INSERT OR IGNORE INTO provider_accounting_warnings VALUES(?1,?2)",
                params![scan, warning],
            )?;
            Ok(())
        })
    }
    pub fn warnings(&self, scan: &str) -> Result<Vec<String>> {
        self.db.with_connection(|db|{Ok(db.prepare("SELECT warning FROM provider_accounting_warnings WHERE scan_id=?1 ORDER BY warning")?.query_map([scan],|r|r.get(0))?.collect::<rusqlite::Result<_>>()?)})
    }
    pub fn enqueue(&self, entry: &OutboxEntry) -> Result<()> {
        self.db.with_connection(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            settle_local(&tx, entry, entry.charge.as_ref().and_then(|c| c.amount))?;
            tx.execute("INSERT INTO provider_accounting_outbox VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET document=excluded.document", params![entry.request.id, serde_json::to_string(entry)?])?;
            tx.commit()?;
            Ok(())
        })
    }
    pub fn outbox(&self) -> Result<Vec<OutboxEntry>> {
        self.db.with_connection(|db| {
            let rows = db
                .prepare("SELECT document FROM provider_accounting_outbox ORDER BY rowid")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .map(|s| Ok(serde_json::from_str(&s)?))
                .collect()
        })
    }
    pub fn acknowledge(&self, id: &str) -> Result<()> {
        self.db.with_connection(|db| {
            db.execute("DELETE FROM provider_accounting_outbox WHERE id=?1", [id])?;
            Ok(())
        })
    }
    pub fn save_allocation(&self, group: &str, target: &PacingTarget) -> Result<()> {
        self.db.with_connection(|db|{db.execute("INSERT INTO provider_accounting_allocations VALUES(?1,?2) ON CONFLICT(group_id) DO UPDATE SET document=excluded.document",params![group,serde_json::to_string(target)?])?;Ok(())})
    }
    pub fn allocation(&self, group: &str) -> Result<Option<PacingTarget>> {
        self.db.with_connection(|db| {
            let text = db
                .query_row(
                    "SELECT document FROM provider_accounting_allocations WHERE group_id=?1",
                    [group],
                    |r| r.get::<_, String>(0),
                )
                .optional()?;
            Ok(text.map(|s| serde_json::from_str(&s)).transpose()?)
        })
    }
    pub fn local_admit(
        &self,
        group: &str,
        target: Option<&PacingTarget>,
        now: u64,
    ) -> Result<Option<u64>> {
        self.db.with_connection(|db|{
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (at,last)=tx.query_row("SELECT next_at_ms,last_cost FROM provider_accounting_local WHERE group_id=?1",[group],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?))).optional()?.unwrap_or((0,0));
            if at.max(0) as u64>now{return Ok(None)}
            let estimate=if last>0{last as u64}else{target.map_or(0,|t|(amount(t)/60).max(1))};
            let next=now.saturating_add(target.map_or(1000,|t|duration(t,estimate)));
            tx.execute("INSERT INTO provider_accounting_local VALUES(?1,?2,?3) ON CONFLICT(group_id) DO UPDATE SET next_at_ms=excluded.next_at_ms",params![group,i64::try_from(next)?,last])?;
            tx.commit()?;Ok(Some(estimate))
        })
    }
    pub fn local_settle(&self, entry: &OutboxEntry, cost: Option<u64>) -> Result<()> {
        self.db.with_connection(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            settle_local(&tx, entry, cost)?;
            tx.commit()?;
            Ok(())
        })
    }
    pub fn claim_probe(&self, group: &str, now: u64) -> Result<bool> {
        self.db.with_connection(|db|{Ok(db.execute("INSERT INTO provider_accounting_probes VALUES(?1,?2) ON CONFLICT(group_id) DO UPDATE SET at_ms=excluded.at_ms WHERE at_ms<=?3",params![group,i64::try_from(now)?,i64::try_from(now.saturating_sub(30000))?])?==1)})
    }
}
