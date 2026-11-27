use anyhow::{Context, Result, ensure};
use beale_contracts::swarm_worker::{ExecutionIdentity, LeaseGrant};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseState {
    AwaitingGrant,
    Active,
    TerminationUnconfirmed,
    Stopped,
}

/// Host control traffic is allowed without a model turn. This ticket binds a
/// response to its send time and ordering; workers cannot derive it from disk.
pub struct Renewal {
    pub request_id: String,
    identity: ExecutionIdentity,
    sequence: u64,
    sent: Instant,
    owner: Uuid,
}

/// Monotonic, process-local execution authority. Recovery always starts awaiting
/// a fresh grant; persisted checkpoints and wall-clock timestamps grant nothing.
pub struct Lease {
    identity: ExecutionIdentity,
    state: LeaseState,
    deadline: Option<Instant>,
    next_sequence: u64,
    applied_sequence: u64,
    owner: Uuid,
}

impl Lease {
    pub fn new(identity: ExecutionIdentity) -> Result<Self> {
        identity.validate().map_err(anyhow::Error::msg)?;
        Ok(Self {
            identity,
            state: LeaseState::AwaitingGrant,
            deadline: None,
            next_sequence: 1,
            applied_sequence: 0,
            owner: Uuid::new_v4(),
        })
    }

    pub fn state(&mut self, now: Instant) -> LeaseState {
        if self.state == LeaseState::Active && self.deadline.is_none_or(|deadline| now >= deadline)
        {
            self.stop();
        }
        self.state
    }

    pub fn begin_renewal(&mut self, now: Instant) -> Result<Renewal> {
        ensure!(
            matches!(
                self.state(now),
                LeaseState::AwaitingGrant | LeaseState::Active
            ),
            "execution cannot renew after expiry or cancellation"
        );
        let sequence = self.next_sequence;
        self.next_sequence = sequence
            .checked_add(1)
            .context("renewal sequence overflow")?;
        Ok(Renewal {
            request_id: Uuid::new_v4().to_string(),
            identity: self.identity.clone(),
            sequence,
            sent: now,
            owner: self.owner,
        })
    }

    pub fn apply(&mut self, renewal: Renewal, grant: &LeaseGrant, now: Instant) -> Result<()> {
        ensure!(
            matches!(
                self.state(now),
                LeaseState::AwaitingGrant | LeaseState::Active
            ),
            "late grant cannot resurrect execution"
        );
        grant.validate().map_err(anyhow::Error::msg)?;
        ensure!(
            renewal.owner == self.owner
                && renewal.identity == self.identity
                && grant.identity == self.identity
                && grant.request_id == renewal.request_id
                && renewal.sequence > self.applied_sequence
                && renewal.sequence < self.next_sequence
                && now >= renewal.sent,
            "foreign or out-of-order lease grant"
        );
        let deadline = renewal
            .sent
            .checked_add(Duration::from_millis(grant.valid_for_ms))
            .context("lease deadline overflow")?;
        if deadline <= now {
            self.stop();
            anyhow::bail!("lease response arrived after its execution deadline");
        }
        // A later request may shorten a lease. Never retain the previous larger
        // value when the leader has explicitly reduced its grant.
        self.deadline = Some(deadline);
        self.applied_sequence = renewal.sequence;
        self.state = LeaseState::Active;
        Ok(())
    }

    pub fn dispatch_deadline(&mut self, now: Instant) -> Result<Instant> {
        ensure!(
            self.state(now) == LeaseState::Active,
            "worker execution has no live lease"
        );
        self.deadline.context("worker lease has no deadline")
    }

    /// Fences new dispatch immediately. The supervisor still must terminate its
    /// managed processes and obtain confirmation before releasing ownership.
    pub fn stop(&mut self) {
        if self.state != LeaseState::Stopped {
            self.state = LeaseState::TerminationUnconfirmed;
        }
        self.deadline = None;
    }

    /// Called only with confirmation from the owning process/backend supervisor.
    pub fn confirm_termination(&mut self) -> Result<()> {
        ensure!(
            self.state == LeaseState::TerminationUnconfirmed || self.state == LeaseState::Stopped,
            "execution must be fenced before termination confirmation"
        );
        self.state = LeaseState::Stopped;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> ExecutionIdentity {
        ExecutionIdentity {
            workspace_id: "workspace-example".into(),
            scan_id: "scan-example".into(),
            agent_id: "agent-example".into(),
            attempt_id: "attempt-example".into(),
            generation: 1,
        }
    }
    fn grant(renewal: &Renewal, ttl: u64) -> LeaseGrant {
        LeaseGrant {
            identity: identity(),
            request_id: renewal.request_id.clone(),
            valid_for_ms: ttl,
        }
    }

    #[test]
    fn execution_requires_fresh_grant_and_latency_consumes_the_lease() {
        let now = Instant::now();
        let mut lease = Lease::new(identity()).unwrap();
        assert!(lease.dispatch_deadline(now).is_err());
        let renewal = lease.begin_renewal(now).unwrap();
        let grant = grant(&renewal, 1000);
        lease
            .apply(renewal, &grant, now + Duration::from_millis(300))
            .unwrap();
        assert_eq!(
            lease
                .dispatch_deadline(now + Duration::from_millis(999))
                .unwrap(),
            now + Duration::from_secs(1)
        );
        assert!(
            lease
                .dispatch_deadline(now + Duration::from_secs(1))
                .is_err()
        );
        assert_eq!(
            lease.state(now + Duration::from_secs(1)),
            LeaseState::TerminationUnconfirmed
        );
        assert!(lease.begin_renewal(now + Duration::from_secs(1)).is_err());
        let mut restarted = Lease::new(identity()).unwrap();
        assert!(restarted.dispatch_deadline(now).is_err());
    }

    #[test]
    fn late_or_wrong_identity_grants_never_start_execution() {
        let now = Instant::now();
        for field in 0..7 {
            let mut lease = Lease::new(identity()).unwrap();
            let renewal = lease.begin_renewal(now).unwrap();
            let mut reply = grant(&renewal, 1000);
            match field {
                0 => reply.identity.scan_id = "other-scan".into(),
                1 => reply.identity.workspace_id = "other-workspace".into(),
                2 => reply.identity.agent_id = "other-agent".into(),
                3 => reply.identity.attempt_id = "other-attempt".into(),
                4 => reply.identity.generation += 1,
                5 => reply.request_id = "other-request".into(),
                _ => reply.valid_for_ms = 0,
            }
            assert!(lease.apply(renewal, &reply, now).is_err());
            assert!(lease.dispatch_deadline(now).is_err());
        }
        let mut lease = Lease::new(identity()).unwrap();
        let renewal = lease.begin_renewal(now).unwrap();
        let reply = grant(&renewal, 1000);
        assert!(
            lease
                .apply(renewal, &reply, now + Duration::from_secs(1))
                .is_err()
        );
        assert!(lease.begin_renewal(now + Duration::from_secs(1)).is_err());
    }

    #[test]
    fn reordered_renewals_and_cancel_during_reconnect_cannot_extend_authority() {
        let now = Instant::now();
        let mut lease = Lease::new(identity()).unwrap();
        let old = lease.begin_renewal(now).unwrap();
        let old_grant = grant(&old, 10000);
        let new = lease.begin_renewal(now).unwrap();
        let new_grant = grant(&new, 1000);
        lease.apply(new, &new_grant, now).unwrap();
        assert!(lease.apply(old, &old_grant, now).is_err());
        assert_eq!(
            lease.dispatch_deadline(now).unwrap(),
            now + Duration::from_secs(1)
        );
        let pending = lease.begin_renewal(now).unwrap();
        let pending_grant = grant(&pending, 10000);
        lease.stop();
        assert!(lease.apply(pending, &pending_grant, now).is_err());
        assert_eq!(lease.state(now), LeaseState::TerminationUnconfirmed);
        lease.confirm_termination().unwrap();
        lease.stop();
        assert_eq!(lease.state(now), LeaseState::Stopped);
        assert!(lease.begin_renewal(now).is_err());
    }

    #[test]
    fn expiry_is_not_termination_and_an_inflight_renewal_cannot_revive_it() {
        let now = Instant::now();
        let mut lease = Lease::new(identity()).unwrap();
        assert!(lease.confirm_termination().is_err());
        let first = lease.begin_renewal(now).unwrap();
        let reply = grant(&first, 1000);
        lease.apply(first, &reply, now).unwrap();
        let second = lease
            .begin_renewal(now + Duration::from_millis(500))
            .unwrap();
        let reply = grant(&second, 1000);
        assert!(
            lease
                .apply(second, &reply, now + Duration::from_secs(1))
                .is_err()
        );
        assert_eq!(
            lease.state(now + Duration::from_secs(1)),
            LeaseState::TerminationUnconfirmed
        );
    }

    #[test]
    fn another_worker_process_cannot_reuse_a_prior_process_renewal_ticket() {
        let now = Instant::now();
        let mut previous = Lease::new(identity()).unwrap();
        let renewal = previous.begin_renewal(now).unwrap();
        let reply = grant(&renewal, 1000);
        let mut restarted = Lease::new(identity()).unwrap();
        let _fresh_request = restarted.begin_renewal(now).unwrap();
        assert!(restarted.apply(renewal, &reply, now).is_err());
        assert!(restarted.dispatch_deadline(now).is_err());
    }
}
