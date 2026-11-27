use beale_contracts::{pacing::*, swarms::*};
use beale_storage::{
    HostDatabase,
    fleet_coordination::FleetCoordinationStore,
    provider_accounting::{AccountingStore, OutboxEntry},
};

struct Fixture {
    db: HostDatabase,
    store: AccountingStore,
    binding: AccountBinding,
    root: TemporaryDirectory,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("beale-accounting-test-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let store = AccountingStore::open(db.clone()).unwrap();
        let binding = AccountBinding {
            group_id: "account-example".into(),
            provider_id: "provider-1".into(),
            owner_host_id: store.host_id().unwrap(),
            generation: 1,
            participants: vec!["leader-a".into(), "leader-b".into()],
            limited_identity: false,
        };
        store.bind("example", binding.clone()).unwrap();
        Self {
            root: TemporaryDirectory(root),
            db,
            store,
            binding,
        }
    }
    fn call(&self, id: &str, scan: &str) -> Admission {
        Admission {
            id: id.into(),
            group_id: self.binding.group_id.clone(),
            generation: 1,
            participant_id: "leader-a".into(),
            scan_id: scan.into(),
            target: Some(PacingTarget {
                budget_group_id: self.binding.group_id.clone(),
                interval_seconds: 60,
                amount: PacingAmount::Currency {
                    currency: "USD".into(),
                    micros: 6000,
                },
            }),
            class: CallClass::Research,
        }
    }
}
struct TemporaryDirectory(std::path::PathBuf);
impl std::ops::Deref for TemporaryDirectory {
    type Target = std::path::Path;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let path = self.0.canonicalize().unwrap();
        assert!(path.starts_with(std::env::temp_dir().canonicalize().unwrap()));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("beale-accounting-test-")
        );
        let _ = std::fs::remove_dir_all(path);
    }
}

fn charge(id: &str, amount: Option<u64>) -> Charge {
    Charge {
        attempt_id: id.into(),
        amount,
        quality: if amount.is_some() {
            CostQuality::Estimated
        } else {
            CostQuality::Unknown
        },
        uncertain: amount.is_none(),
        input_tokens: Some(100),
        output_tokens: Some(10),
        cache_read_tokens: None,
    }
}

#[test]
fn fake_clock_delay_retry_identity_and_restart_debt() {
    let f = Fixture::new();
    let first = f.call("first", "scan-a");
    assert!(f.store.admit(&first, 1000).unwrap().admitted);
    f.store.settle(&first, &charge("first", Some(600))).unwrap();
    f.store.settle(&first, &charge("first", Some(600))).unwrap();
    assert!(f.store.settle(&first, &charge("first", Some(601))).is_err());
    let second = f.call("second", "scan-a");
    let reopened = AccountingStore::open(HostDatabase::open(&f.root).unwrap()).unwrap();
    assert!(!reopened.admit(&second, 6999).unwrap().admitted);
    assert!(reopened.admit(&second, 7000).unwrap().admitted);
    reopened.settle(&second, &charge("second", None)).unwrap();
    let state = reopened.snapshot(&f.binding.group_id).unwrap();
    assert_eq!(state.reservations.len(), 2);
    assert_eq!(state.reservations[1].charge.as_ref().unwrap().amount, None);
    assert_eq!(state.next_at_ms, 13000);
}

#[test]
fn two_scans_and_leaders_share_allowance_and_supporting_calls_alternate() {
    let f = Fixture::new();
    let first = f.call("first", "scan-a");
    f.store.admit(&first, 0).unwrap();
    let mut second = f.call("second", "scan-b");
    second.participant_id = "leader-b".into();
    let mut review = f.call("review", "scan-a");
    review.class = CallClass::Supporting;
    assert!(!f.store.admit(&second, 1).unwrap().admitted);
    assert!(!f.store.admit(&review, 1).unwrap().admitted);
    assert!(!f.store.admit(&second, 1000).unwrap().admitted);
    let reply = f.store.admit(&review, 1000).unwrap();
    assert!(reply.admitted);
    assert_eq!(
        beale_storage::provider_accounting::amount(&reply.allocation.unwrap()),
        6000
    );
    f.store
        .settle(&review, &charge("review", Some(100)))
        .unwrap();
    assert!(f.store.admit(&second, 2000).unwrap().admitted);
    assert_eq!(
        f.store
            .snapshot(&f.binding.group_id)
            .unwrap()
            .reservations
            .iter()
            .filter(|r| r.request.class == CallClass::Supporting && r.charge.is_some())
            .count(),
        1
    );
}

#[test]
fn named_subscription_windows_reset_and_external_usage_delay() {
    let f = Fixture::new();
    let mut call = f.call("first", "scan-a");
    call.target.as_mut().unwrap().amount = PacingAmount::Subscription {
        window_id: "primary".into(),
        basis_points: 600,
    };
    let obs = |used, reset, now| Observation {
        window_id: "primary".into(),
        used_basis_points: used,
        resets_at_ms: reset,
        observed_at_ms: now,
    };
    f.store
        .observe(&f.binding.group_id, 1, obs(9000, 100000, 0), 0)
        .unwrap();
    assert!(f.store.admit(&call, 0).unwrap().admitted);
    // Five percentage points, not five percent of the remaining ten points.
    f.store
        .observe(&f.binding.group_id, 1, obs(9500, 100000, 100), 100)
        .unwrap();
    let mut next = call.clone();
    next.id = "second".into();
    assert!(!f.store.admit(&next, 1000).unwrap().admitted);
    f.store
        .observe(&f.binding.group_id, 1, obs(10000, 100000, 200), 200)
        .unwrap();
    assert!(!f.store.admit(&next, 99999).unwrap().admitted);
    f.store
        .observe(&f.binding.group_id, 1, obs(0, 200000, 100000), 100000)
        .unwrap();
    assert!(f.store.admit(&next, 100000).unwrap().admitted);
    assert!(
        f.store
            .observe(&f.binding.group_id, 1, obs(0, 200000, 50), 100000)
            .is_err()
    );
}

#[test]
fn stale_missing_and_limited_account_measurements_are_visible() {
    let f = Fixture::new();
    let mut binding = f.binding.clone();
    binding.group_id = "limited-example".into();
    binding.limited_identity = true;
    f.store.bind("limited", binding.clone()).unwrap();
    let mut call = f.call("first", "scan-a");
    call.group_id = binding.group_id.clone();
    let t = call.target.as_mut().unwrap();
    t.budget_group_id = binding.group_id.clone();
    t.amount = PacingAmount::Subscription {
        window_id: "primary".into(),
        basis_points: 500,
    };
    let reply = f.store.admit(&call, 70000).unwrap();
    assert!(reply.admitted);
    assert!(
        reply
            .warnings
            .iter()
            .any(|w| w.contains("stale or unavailable"))
    );
    assert!(reply.warnings.iter().any(|w| w.contains("cross-host")));
    assert!(
        reply
            .warnings
            .iter()
            .any(|w| w.contains("not a spending cap"))
    );
}

fn unpaced_shared_allowance_waits_for_reset(class: CallClass) {
    for reset_at in [60_000, 120_000] {
        let f = Fixture::new();
        let mut paced = f.call("paced", "scan-paced");
        paced.target.as_mut().unwrap().amount = PacingAmount::Subscription {
            window_id: "example-window".into(),
            basis_points: 600,
        };
        assert!(f.store.admit(&paced, 0).unwrap().admitted);
        let observe_exhausted = |at| {
            f.store
                .observe(
                    &f.binding.group_id,
                    1,
                    Observation {
                        window_id: "example-window".into(),
                        used_basis_points: 10000,
                        resets_at_ms: reset_at,
                        observed_at_ms: at,
                    },
                    at,
                )
                .unwrap();
        };
        observe_exhausted(100);
        // Existing inference remains admitted and may finish despite exhaustion.
        assert!(f.store.admit(&paced, 100).unwrap().admitted);
        f.store.settle(&paced, &charge(&paced.id, None)).unwrap();
        let mut call = f.call("unpaced", "scan-unpaced");
        call.participant_id = "leader-b".into();
        call.target = None;
        call.class = class.clone();
        let reply = f.store.admit(&call, 1000).unwrap();
        assert!(!reply.admitted);
        assert_eq!(reply.retry_at_ms, reset_at);
        assert!(reply.allocation.is_some());
        if reset_at > 60_000 {
            // A fresh observation still applies when no paced rate lease is active.
            observe_exhausted(70_000);
            let reply = f.store.admit(&call, 70_000).unwrap();
            assert!(!reply.admitted);
            assert_eq!(reply.retry_at_ms, reset_at);
            assert!(reply.allocation.is_none());
        }
        let reopened = AccountingStore::open(HostDatabase::open(&f.root).unwrap()).unwrap();
        let reply = reopened.admit(&call, reset_at - 1).unwrap();
        assert!(!reply.admitted);
        // retry_at retains the scheduler's minimum 250ms retry interval.
        assert!(reply.retry_at_ms >= reset_at);
        let state = reopened.snapshot(&call.group_id).unwrap();
        assert_eq!(state.reservations.len(), 2);
        let waiting = state
            .reservations
            .iter()
            .find(|r| r.request.id == call.id)
            .unwrap();
        assert!(!waiting.admitted && waiting.charge.is_none());
        assert_eq!(waiting.estimate, 0);
        assert!(waiting.request.target.is_none());
        assert!(
            state
                .reservations
                .iter()
                .find(|r| r.request.id == paced.id)
                .unwrap()
                .charge
                .is_some()
        );
        assert!(reopened.admit(&call, reset_at).unwrap().admitted);
        // Retrying a granted ticket does not create another reservation or debt.
        let granted = reopened.snapshot(&call.group_id).unwrap();
        assert!(reopened.admit(&call, reset_at).unwrap().admitted);
        assert_eq!(
            serde_json::to_value(&granted).unwrap(),
            serde_json::to_value(reopened.snapshot(&call.group_id).unwrap()).unwrap()
        );
    }
}

#[test]
fn unpaced_research_waits_for_shared_allowance_reset() {
    unpaced_shared_allowance_waits_for_reset(CallClass::Research);
}

#[test]
fn unpaced_supporting_call_waits_for_shared_allowance_reset() {
    unpaced_shared_allowance_waits_for_reset(CallClass::Supporting);
}

fn bounded_handover_keeps_shared_allowance(class: CallClass) {
    let f = Fixture::new();
    let mut paced = f.call("paced", "scan-paced");
    paced.target.as_mut().unwrap().amount = PacingAmount::Subscription {
        window_id: "example-window".into(),
        basis_points: 600,
    };
    assert!(f.store.admit(&paced, 0).unwrap().admitted);
    f.store.settle(&paced, &charge(&paced.id, None)).unwrap();
    for i in 0..128 {
        let mut r = f.call(&format!("unpaced-{i}"), "scan-unpaced");
        r.target = None;
        assert!(f.store.admit(&r, 70_000 + i).unwrap().admitted);
        f.store.settle(&r, &charge(&r.id, None)).unwrap();
    }
    f.store
        .observe(
            &f.binding.group_id,
            1,
            Observation {
                window_id: "example-window".into(),
                used_basis_points: 10000,
                resets_at_ms: 120_000,
                observed_at_ms: 71_000,
            },
            71_000,
        )
        .unwrap();
    let mut call = f.call("before-transfer", "scan-unpaced");
    call.target = None;
    call.class = class;
    assert!(!f.store.admit(&call, 71_000).unwrap().admitted);
    f.store.cancel(&call).unwrap();
    for participant in &f.binding.participants {
        f.store.drain(&f.binding.group_id, 1, participant).unwrap();
    }
    let transfer = f.store.freeze(&f.binding.group_id, 1).unwrap();
    assert_eq!(transfer.reservations.len(), 128);
    assert_eq!(transfer.omitted_reservations, 2);
    assert!(
        transfer
            .reservations
            .iter()
            .all(|r| r.request.target.is_none() && r.aggregate_target.is_none())
    );
    assert_eq!(
        transfer.unit,
        Some(AccountingUnit::Subscription {
            window_id: "example-window".into()
        })
    );
    let transfer: AccountingSnapshot =
        serde_json::from_slice(&serde_json::to_vec(&transfer).unwrap()).unwrap();
    let root = f.root.join("next-owner");
    let next = AccountingStore::open(HostDatabase::open(&root).unwrap()).unwrap();
    next.import(transfer, &next.host_id().unwrap()).unwrap();
    let next = AccountingStore::open(HostDatabase::open(&root).unwrap()).unwrap();
    call.id = "after-transfer".into();
    assert!(
        next.admit(&call, 72_000).is_err(),
        "old owner generation must remain fenced"
    );
    call.generation = 2;
    let reply = next.admit(&call, 72_000).unwrap();
    assert!(!reply.admitted);
    assert_eq!(reply.retry_at_ms, 120_000);
    assert!(
        reply.allocation.is_none(),
        "preserving units must not reactivate expired rates"
    );
    assert!(!next.admit(&call, 119_999).unwrap().admitted);
    let before_reset = next.snapshot(&call.group_id).unwrap();
    assert_eq!(before_reset.observations[0].used_basis_points, 10000);
    let waiting = before_reset
        .reservations
        .iter()
        .find(|r| r.request.id == call.id)
        .unwrap();
    assert!(!waiting.admitted && waiting.charge.is_none());
    assert!(next.admit(&call, 120_000).unwrap().admitted);
    let granted = next.snapshot(&call.group_id).unwrap();
    assert!(next.admit(&call, 120_000).unwrap().admitted);
    assert_eq!(
        serde_json::to_value(&granted).unwrap(),
        serde_json::to_value(next.snapshot(&call.group_id).unwrap()).unwrap()
    );
}

#[test]
fn bounded_handover_keeps_unpaced_research_waiting_until_allowance_reset() {
    bounded_handover_keeps_shared_allowance(CallClass::Research);
}

#[test]
fn bounded_handover_keeps_unpaced_supporting_calls_waiting_until_allowance_reset() {
    bounded_handover_keeps_shared_allowance(CallClass::Supporting);
}

#[test]
fn handover_validates_persistent_units_and_cannot_erase_or_change_them() {
    for amount in [
        PacingAmount::Currency {
            currency: "USD".into(),
            micros: 6000,
        },
        PacingAmount::Credits { micros: 6000 },
        PacingAmount::Subscription {
            window_id: "example-window".into(),
            basis_points: 600,
        },
    ] {
        let f = Fixture::new();
        let mut call = f.call("known-unit", "scan-a");
        call.target.as_mut().unwrap().amount = amount;
        f.store
            .reconcile(&call, &charge(&call.id, None), 1000)
            .unwrap();
        for participant in &f.binding.participants {
            f.store.drain(&f.binding.group_id, 1, participant).unwrap();
        }
        let transfer = f.store.freeze(&f.binding.group_id, 1).unwrap();
        let before = serde_json::to_value(f.store.snapshot(&f.binding.group_id).unwrap()).unwrap();
        for replacement in [
            None,
            Some(AccountingUnit::Currency {
                currency: "eur".into(),
            }),
            Some(AccountingUnit::Subscription {
                window_id: "".into(),
            }),
            Some(AccountingUnit::Currency {
                currency: "EUR".into(),
            }),
        ] {
            let mut invalid = transfer.clone();
            invalid.unit = replacement;
            assert!(f.store.import(invalid.clone(), "next-owner").is_err());
            // Even when bounded history has no unit-bearing entries, an
            // importing host's existing identity cannot be cleared or changed.
            invalid.reservations.clear();
            assert!(f.store.import(invalid, "next-owner").is_err());
            assert_eq!(
                before,
                serde_json::to_value(f.store.snapshot(&f.binding.group_id).unwrap()).unwrap()
            );
        }
        let root = f.root.join("next-owner");
        let next = AccountingStore::open(HostDatabase::open(&root).unwrap()).unwrap();
        let mut bounded = transfer.clone();
        bounded.reservations.clear();
        bounded.omitted_reservations += 1;
        next.import(
            serde_json::from_slice(&serde_json::to_vec(&bounded).unwrap()).unwrap(),
            &next.host_id().unwrap(),
        )
        .unwrap();
        assert_eq!(
            next.snapshot(&f.binding.group_id).unwrap().unit,
            transfer.unit
        );
        let mut wrong = f.call("wrong-unit", "scan-b");
        wrong.generation = 2;
        wrong.target.as_mut().unwrap().amount = PacingAmount::Currency {
            currency: "EUR".into(),
            micros: 6000,
        };
        let before = serde_json::to_value(next.snapshot(&f.binding.group_id).unwrap()).unwrap();
        assert!(next.admit(&wrong, 2000).is_err());
        assert!(
            next.reconcile(&wrong, &charge(&wrong.id, None), 2000)
                .is_err()
        );
        assert_eq!(
            before,
            serde_json::to_value(next.snapshot(&f.binding.group_id).unwrap()).unwrap()
        );
        wrong.target = call.target.clone();
        assert!(next.admit(&wrong, 3000).unwrap().admitted);
    }
}

#[test]
fn cancel_receipts_do_not_establish_units_but_accepted_waiters_do() {
    let f = Fixture::new();
    let canceled = f.call("canceled", "scan-a");
    f.store.cancel(&canceled).unwrap();
    assert!(
        f.store
            .snapshot(&f.binding.group_id)
            .unwrap()
            .unit
            .is_none()
    );
    let mut admitted = f.call("admitted", "scan-b");
    admitted.target.as_mut().unwrap().amount = PacingAmount::Credits { micros: 6000 };
    assert!(f.store.admit(&admitted, 1000).unwrap().admitted);
    assert_eq!(
        f.store.snapshot(&f.binding.group_id).unwrap().unit,
        Some(AccountingUnit::Credits)
    );

    let f = Fixture::new();
    let mut waiting = f.call("waiting", "scan-a");
    waiting.target.as_mut().unwrap().amount = PacingAmount::Subscription {
        window_id: "example-window".into(),
        basis_points: 600,
    };
    f.store
        .observe(
            &f.binding.group_id,
            1,
            Observation {
                window_id: "example-window".into(),
                used_basis_points: 10000,
                resets_at_ms: 60_000,
                observed_at_ms: 0,
            },
            0,
        )
        .unwrap();
    assert!(!f.store.admit(&waiting, 1000).unwrap().admitted);
    f.store.cancel(&waiting).unwrap();
    assert_eq!(
        f.store.snapshot(&f.binding.group_id).unwrap().unit,
        Some(AccountingUnit::Subscription {
            window_id: "example-window".into()
        })
    );
    let mut unpaced = f.call("unpaced", "scan-b");
    unpaced.target = None;
    assert!(!f.store.admit(&unpaced, 2000).unwrap().admitted);
}

#[test]
fn unpaced_allowance_checks_keep_named_window_and_stale_measurement_boundaries() {
    for stale in [false, true] {
        let f = Fixture::new();
        let mut paced = f.call("paced", "scan-paced");
        paced.target.as_mut().unwrap().amount = PacingAmount::Subscription {
            window_id: "example-window".into(),
            basis_points: 600,
        };
        assert!(f.store.admit(&paced, 0).unwrap().admitted);
        f.store.settle(&paced, &charge(&paced.id, None)).unwrap();
        f.store
            .observe(
                &f.binding.group_id,
                1,
                Observation {
                    window_id: if stale {
                        "example-window"
                    } else {
                        "unrelated-window"
                    }
                    .into(),
                    used_basis_points: 10000,
                    resets_at_ms: 120_000,
                    observed_at_ms: 100,
                },
                100,
            )
            .unwrap();
        let mut call = f.call("unpaced", "scan-unpaced");
        call.target = None;
        let reply = f
            .store
            .admit(&call, if stale { 70_000 } else { 1000 })
            .unwrap();
        assert!(reply.admitted);
        assert!(
            reply
                .warnings
                .iter()
                .any(|w| w.contains("Allowance measurements are stale or unavailable"))
        );
    }
}

#[test]
fn providers_and_distinct_accounts_are_isolated_and_refresh_uses_stable_identity() {
    let f = Fixture::new();
    let groups = FleetCoordinationStore::open(f.db.clone()).unwrap();
    let a = groups
        .account_group("provider-1", "account-a", "leader-a")
        .unwrap();
    assert_eq!(
        a,
        groups
            .account_group("provider-1", "account-a", "leader-a")
            .unwrap()
    );
    assert_ne!(
        a.id,
        groups
            .account_group("provider-1", "account-b", "leader-a")
            .unwrap()
            .id
    );
    assert_ne!(
        a.id,
        groups
            .account_group("provider-2", "account-a", "leader-a")
            .unwrap()
            .id
    );
    let mut other = f.binding.clone();
    other.group_id = "other-account".into();
    other.provider_id = "provider-2".into();
    f.store.bind("other", other.clone()).unwrap();
    assert!(
        f.store
            .admit(&f.call("first", "scan-a"), 100)
            .unwrap()
            .admitted
    );
    let mut call = f.call("second", "scan-a");
    call.group_id = other.group_id.clone();
    call.target.as_mut().unwrap().budget_group_id = other.group_id;
    assert!(f.store.admit(&call, 100).unwrap().admitted);
}

#[test]
fn explicit_handover_requires_reconciliation_and_fences_old_generations() {
    let f = Fixture::new();
    let r = f.call("first", "scan-a");
    f.store.admit(&r, 0).unwrap();
    assert!(f.store.freeze(&r.group_id, 1).is_err());
    f.store.settle(&r, &charge("first", None)).unwrap();
    assert!(f.store.freeze(&r.group_id, 1).is_err());
    for participant in &f.binding.participants {
        f.store.drain(&r.group_id, 1, participant).unwrap();
    }
    let frozen = f.store.freeze(&r.group_id, 1).unwrap();
    assert!(f.store.admit(&f.call("second", "scan-a"), 1000).is_err());
    f.store.import(frozen, &f.binding.owner_host_id).unwrap();
    assert!(f.store.admit(&f.call("second", "scan-a"), 1000).is_err());
    let mut next = f.call("second", "scan-a");
    next.generation = 2;
    assert!(f.store.admit(&next, 1000).unwrap().admitted);
}

#[test]
fn disconnected_usage_reconciles_once_and_restart_marks_unknown_not_zero() {
    let f = Fixture::new();
    let r = f.call("first", "scan-a");
    f.store
        .enqueue(&OutboxEntry {
            request: r.clone(),
            allocation: r.target.clone(),
            charge: None,
            canceled: false,
            local_admitted: true,
            dispatched: true,
            local_estimate: 100,
            observed_charge: None,
        })
        .unwrap();
    f.store.recover_abandoned().unwrap();
    let e = f.store.outbox().unwrap().remove(0);
    assert_eq!(e.charge.as_ref().unwrap().quality, CostQuality::Unknown);
    f.store
        .reconcile(&r, e.charge.as_ref().unwrap(), 100)
        .unwrap();
    f.store
        .reconcile(&r, e.charge.as_ref().unwrap(), 200)
        .unwrap();
    assert_eq!(f.store.snapshot(&r.group_id).unwrap().reservations.len(), 1);
    f.store.acknowledge(&r.id).unwrap();
    assert!(f.store.outbox().unwrap().is_empty());
    f.store
        .save_allocation(&r.group_id, r.target.as_ref().unwrap())
        .unwrap();
    let reopened = AccountingStore::open(f.db.clone()).unwrap();
    assert_eq!(reopened.allocation(&r.group_id).unwrap(), r.target);
    assert!(
        reopened
            .local_admit(&r.group_id, r.target.as_ref(), 100)
            .unwrap()
            .is_some()
    );
    assert!(
        !reopened
            .local_admit(&r.group_id, r.target.as_ref(), 101)
            .unwrap()
            .is_some()
    );
}

#[test]
fn waiting_cancellation_and_abandoned_tickets_do_not_cancel_active_work() {
    let f = Fixture::new();
    let first = f.call("first", "scan-a");
    f.store.admit(&first, 0).unwrap();
    let waiting = f.call("waiting", "scan-a");
    assert!(!f.store.admit(&waiting, 1).unwrap().admitted);
    f.store.cancel(&waiting).unwrap();
    assert!(!f.store.snapshot(&first.group_id).unwrap().reservations[0].canceled);
    let abandoned = f.call("abandoned", "scan-a");
    assert!(!f.store.admit(&abandoned, 2).unwrap().admitted);
    let fresh = f.call("fresh", "scan-a");
    assert!(f.store.admit(&fresh, 70000).unwrap().admitted);
    assert!(
        f.store
            .snapshot(&first.group_id)
            .unwrap()
            .reservations
            .iter()
            .find(|r| r.request.id == "abandoned")
            .unwrap()
            .canceled
    );
    f.store.settle(&first, &charge("first", Some(100))).unwrap();
}

#[test]
fn malformed_targets_participants_replays_and_unit_changes_are_rejected() {
    let f = Fixture::new();
    let r = f.call("first", "scan-a");
    assert!(f.store.admit(&r, 0).unwrap().admitted);
    let mut bad = r.clone();
    bad.class = CallClass::Supporting;
    assert!(f.store.admit(&bad, 1).is_err());
    bad = r.clone();
    bad.id = "second".into();
    bad.participant_id = "unregistered".into();
    assert!(f.store.admit(&bad, 1).is_err());
    bad = r.clone();
    bad.id = "third".into();
    bad.target.as_mut().unwrap().amount = PacingAmount::Credits { micros: 10 };
    assert!(f.store.admit(&bad, 1).is_err());
    bad = r;
    bad.id = "fourth".into();
    bad.target.as_mut().unwrap().interval_seconds = 0;
    assert!(f.store.admit(&bad, 1).is_err());
}

#[test]
fn degraded_allocations_keep_scan_limits_and_reconcile_known_local_cost_once() {
    use beale_storage::provider_accounting::{conservative_allocation, divided};
    let f = Fixture::new();
    let r = f.call("degraded", "scan-a");
    let target = r.target.clone().unwrap();
    let split = divided(target.clone(), 2);
    assert_eq!(split.interval_seconds, 120);
    assert_eq!(
        conservative_allocation(Some(target.clone()), Some(&split)).unwrap(),
        Some(split.clone())
    );
    let estimate = f
        .store
        .local_admit(&r.group_id, Some(&split), 1000)
        .unwrap()
        .unwrap();
    let entry = OutboxEntry {
        request: r.clone(),
        allocation: Some(split.clone()),
        charge: None,
        canceled: false,
        local_admitted: true,
        dispatched: true,
        local_estimate: estimate,
        observed_charge: None,
    };
    f.store.local_settle(&entry, Some(600)).unwrap();
    f.store.local_settle(&entry, Some(600)).unwrap();
    let reopened = AccountingStore::open(f.db.clone()).unwrap();
    assert!(
        reopened
            .local_admit(&r.group_id, Some(&split), 12999)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        reopened
            .local_admit(&r.group_id, Some(&split), 13000)
            .unwrap(),
        Some(600)
    );
}

#[test]
fn lost_admission_acknowledgement_is_reconciled_as_uncertain_before_handover() {
    let f = Fixture::new();
    let r = f.call("lost-grant", "scan-a");
    f.store.admit(&r, 0).unwrap();
    assert!(f.store.drain(&r.group_id, 1, "leader-a").is_err());
    f.store.cancel(&r).unwrap();
    f.store.cancel(&r).unwrap();
    let state = f.store.snapshot(&r.group_id).unwrap();
    assert_eq!(state.reservations.len(), 1);
    assert_eq!(
        state.reservations[0].charge.as_ref().unwrap().quality,
        CostQuality::Unknown
    );
    f.store.drain(&r.group_id, 1, "leader-a").unwrap();
    assert!(f.store.admit(&f.call("blocked", "scan-a"), 1000).is_err());
    assert!(f.store.freeze(&r.group_id, 1).is_err());
    f.store.drain(&r.group_id, 1, "leader-b").unwrap();
    assert!(f.store.freeze(&r.group_id, 1).is_ok());
}

#[test]
fn handover_is_bounded_preserves_debt_and_rejects_malformed_transfers() {
    let f = Fixture::new();
    for i in 0..140 {
        let r = f.call(&format!("attempt-{i}"), "scan-a");
        assert!(f.store.admit(&r, i * 1000).unwrap().admitted);
        f.store.settle(&r, &charge(&r.id, Some(100))).unwrap();
    }
    for p in &f.binding.participants {
        f.store.drain(&f.binding.group_id, 1, p).unwrap();
    }
    let frozen = f.store.freeze(&f.binding.group_id, 1).unwrap();
    assert_eq!(frozen.reservations.len(), 128);
    assert_eq!(frozen.omitted_reservations, 12);
    assert_eq!(frozen.next_at_ms, 140000);
    let mut invalid = frozen.clone();
    invalid.reservations[0]
        .request
        .target
        .as_mut()
        .unwrap()
        .interval_seconds = 0;
    assert!(f.store.import(invalid, &f.binding.owner_host_id).is_err());
    let mut invalid = frozen.clone();
    invalid.reservations[0]
        .aggregate_target
        .as_mut()
        .unwrap()
        .interval_seconds = 0;
    assert!(f.store.import(invalid, &f.binding.owner_host_id).is_err());
    f.store.import(frozen, &f.binding.owner_host_id).unwrap();
    let mut r = f.call("new-generation", "scan-a");
    r.generation = 2;
    assert!(!f.store.admit(&r, 139999).unwrap().admitted);
    assert!(f.store.admit(&r, 140000).unwrap().admitted);
}

#[test]
fn cancel_before_delayed_admit_must_remain_terminal() {
    let f = Fixture::new();
    let r = f.call("reordered", "scan-a");
    f.store.cancel(&r).unwrap();
    f.store.cancel(&r).unwrap();
    let reopened = AccountingStore::open(HostDatabase::open(&f.root).unwrap()).unwrap();
    let mut changed = r.clone();
    changed.scan_id = "changed-scan".into();
    assert!(reopened.cancel(&changed).is_err());
    assert!(reopened.admit(&r, 1000).is_err());
    for participant in &f.binding.participants {
        reopened.drain(&r.group_id, 1, participant).unwrap();
    }
    // Drain and freeze succeed despite cancellation reaching the host first.
    assert!(reopened.freeze(&r.group_id, 1).is_ok());
    assert!(
        f.store.admit(&r, 1000).is_err(),
        "delayed admission recreated an acknowledged canceled attempt"
    );
}

#[test]
fn unpaced_scan_usage_must_increase_shared_debt() {
    let f = Fixture::new();
    let first = f.call("first", "scan-a");
    assert!(f.store.admit(&first, 0).unwrap().admitted);
    f.store.settle(&first, &charge("first", Some(100))).unwrap();
    let mut unpaced = f.call("unpaced", "scan-b");
    unpaced.target = None;
    assert!(f.store.admit(&unpaced, 1000).unwrap().admitted);
    f.store
        .settle(&unpaced, &charge("unpaced", Some(6000)))
        .unwrap();
    assert_eq!(
        f.store.snapshot(&first.group_id).unwrap().next_at_ms,
        61000,
        "shared target must account for known unpaced usage"
    );
}

#[test]
fn different_scan_targets_must_keep_group_and_scan_debt_separate() {
    let f = Fixture::new();
    let fast = f.call("fast", "scan-fast");
    assert!(f.store.admit(&fast, 0).unwrap().admitted);
    f.store.settle(&fast, &charge("fast", Some(100))).unwrap();
    let mut slow = f.call("slow", "scan-slow");
    slow.target.as_mut().unwrap().amount = PacingAmount::Currency {
        currency: "USD".into(),
        micros: 600,
    };
    assert!(f.store.admit(&slow, 1000).unwrap().admitted);
    f.store.settle(&slow, &charge("slow", Some(600))).unwrap();
    let snapshot = f.store.snapshot(&fast.group_id).unwrap();
    assert_eq!(snapshot.scan_next["leader-a:scan-slow"], 61000);
    assert_eq!(
        snapshot.next_at_ms, 7000,
        "aggregate uses fast target, not slow scan target"
    );
}

#[test]
fn restart_must_apply_observed_local_charge_before_new_admission() {
    let f = Fixture::new();
    let r = f.call("interrupted", "scan-a");
    let estimate = f
        .store
        .local_admit(&r.group_id, r.target.as_ref(), 0)
        .unwrap()
        .unwrap();
    f.store
        .enqueue(&OutboxEntry {
            request: r.clone(),
            allocation: r.target.clone(),
            charge: None,
            canceled: false,
            local_admitted: true,
            dispatched: true,
            local_estimate: estimate,
            observed_charge: Some(charge(&r.id, Some(6000))),
        })
        .unwrap();
    f.store.recover_abandoned().unwrap();
    assert_eq!(
        f.store.outbox().unwrap()[0].charge.as_ref().unwrap().amount,
        Some(6000)
    );
    assert!(
        f.store
            .local_admit(&r.group_id, r.target.as_ref(), 1000)
            .unwrap()
            .is_none(),
        "restart retained known usage but ignored it in local pacing"
    );
}

#[test]
fn handover_must_not_erase_leader_local_prices() {
    let f = Fixture::new();
    let mirror =
        AccountingStore::open(HostDatabase::open(&f.root.join("mirror")).unwrap()).unwrap();
    mirror.bind("example", f.binding.clone()).unwrap();
    let price = Price {
        model: "example-model".into(),
        currency_or_credit: "USD".into(),
        input_micros: 1000,
        output_micros: 2000,
        cached_input_micros: 100,
    };
    mirror.price(&f.binding.group_id, price.clone()).unwrap();
    for p in &f.binding.participants {
        f.store.drain(&f.binding.group_id, 1, p).unwrap();
    }
    let frozen = f.store.freeze(&f.binding.group_id, 1).unwrap();
    mirror.import(frozen, &mirror.host_id().unwrap()).unwrap();
    assert_eq!(
        mirror.snapshot(&f.binding.group_id).unwrap().prices,
        vec![price]
    );
}

#[test]
fn former_authority_must_retain_omitted_history() {
    let f = Fixture::new();
    for i in 0..140 {
        let r = f.call(&format!("history-{i}"), "scan-a");
        assert!(f.store.admit(&r, i * 1000).unwrap().admitted);
        f.store.settle(&r, &charge(&r.id, Some(100))).unwrap();
    }
    for p in &f.binding.participants {
        f.store.drain(&f.binding.group_id, 1, p).unwrap();
    }
    let frozen = f.store.freeze(&f.binding.group_id, 1).unwrap();
    assert_eq!(frozen.reservations.len(), 128);
    f.store.import(frozen, "new-authority").unwrap();
    assert_eq!(
        f.store
            .snapshot(&f.binding.group_id)
            .unwrap()
            .reservations
            .len(),
        140,
        "installing new owner metadata discarded the former authority's full ledger"
    );
}

#[test]
fn aggregate_rate_survives_reopen_expiry_and_duplicate_settlement() {
    let f = Fixture::new();
    let fast = f.call("fast", "scan-fast");
    f.store.admit(&fast, 0).unwrap();
    f.store.settle(&fast, &charge(&fast.id, Some(100))).unwrap();
    let mut slow = f.call("slow", "scan-slow");
    slow.target.as_mut().unwrap().amount = PacingAmount::Currency {
        currency: "USD".into(),
        micros: 600,
    };
    assert!(f.store.admit(&slow, 1000).unwrap().admitted);
    let reopened = AccountingStore::open(HostDatabase::open(&f.root).unwrap()).unwrap();
    // Later activity expires the former fast target, but not the granted rate.
    let mut later = slow.clone();
    later.id = "later".into();
    reopened.admit(&later, 70000).unwrap();
    let before = reopened.snapshot(&slow.group_id).unwrap().next_at_ms;
    reopened
        .settle(&slow, &charge(&slow.id, Some(600)))
        .unwrap();
    reopened
        .settle(&slow, &charge(&slow.id, Some(600)))
        .unwrap();
    assert_eq!(
        reopened.snapshot(&slow.group_id).unwrap().next_at_ms,
        before + 5000
    );
}

#[test]
fn disconnected_unpaced_usage_reconciles_against_active_group_once() {
    let f = Fixture::new();
    let paced = f.call("paced", "scan-a");
    f.store.admit(&paced, 0).unwrap();
    f.store
        .settle(&paced, &charge(&paced.id, Some(100)))
        .unwrap();
    let mut offline = f.call("offline", "scan-b");
    offline.target = None;
    f.store
        .reconcile(&offline, &charge(&offline.id, Some(6000)), 1000)
        .unwrap();
    f.store
        .reconcile(&offline, &charge(&offline.id, Some(6000)), 2000)
        .unwrap();
    assert_eq!(f.store.snapshot(&paced.group_id).unwrap().next_at_ms, 61000);
}

#[test]
fn outbox_and_local_debt_commit_atomically_and_retry_without_double_charge() {
    let f = Fixture::new();
    let r = f.call("atomic", "scan-a");
    let estimate = f
        .store
        .local_admit(&r.group_id, r.target.as_ref(), 0)
        .unwrap()
        .unwrap();
    let entry = OutboxEntry {
        request: r.clone(),
        allocation: r.target.clone(),
        charge: Some(charge(&r.id, Some(6000))),
        canceled: false,
        local_admitted: true,
        dispatched: true,
        local_estimate: estimate,
        observed_charge: None,
    };
    f.db.with_connection(|db|{db.execute_batch("CREATE TRIGGER fail_outbox BEFORE INSERT ON provider_accounting_outbox BEGIN SELECT RAISE(FAIL,'synthetic outbox failure'); END;")?;Ok(())}).unwrap();
    assert!(f.store.enqueue(&entry).is_err());
    let state =
        f.db.with_connection(|db| {
            Ok((
                db.query_row(
                    "SELECT next_at_ms FROM provider_accounting_local",
                    [],
                    |r| r.get::<_, i64>(0),
                )?,
                db.query_row(
                    "SELECT COUNT(*) FROM provider_accounting_local_charges",
                    [],
                    |r| r.get::<_, i64>(0),
                )?,
            ))
        })
        .unwrap();
    assert_eq!(state, (1000, 0));
    assert!(f.store.outbox().unwrap().is_empty());
    f.db.with_connection(|db| {
        db.execute_batch("DROP TRIGGER fail_outbox")?;
        Ok(())
    })
    .unwrap();
    f.store.enqueue(&entry).unwrap();
    f.store.enqueue(&entry).unwrap();
    f.store.recover_abandoned().unwrap();
    assert!(
        f.store
            .local_admit(&r.group_id, r.target.as_ref(), 59999)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.store
            .local_admit(&r.group_id, r.target.as_ref(), 60000)
            .unwrap(),
        Some(6000)
    );
}

#[test]
fn two_host_handover_roundtrip_preserves_local_prices_full_history_and_debt() {
    use beale_storage::provider_accounting::bounded_snapshot;
    let f = Fixture::new();
    let other =
        AccountingStore::open(HostDatabase::open(&f.root.join("other-host")).unwrap()).unwrap();
    let a = f.store.host_id().unwrap();
    let b = other.host_id().unwrap();
    let binding = AccountBinding {
        group_id: "handover-example".into(),
        provider_id: "provider-1".into(),
        owner_host_id: a.clone(),
        generation: 1,
        participants: vec![a.clone(), b.clone()],
        limited_identity: false,
    };
    f.store.bind("handover", binding.clone()).unwrap();
    other.bind("handover", binding.clone()).unwrap();
    let price_a = Price {
        model: "example-model".into(),
        currency_or_credit: "USD".into(),
        input_micros: 1000,
        output_micros: 2000,
        cached_input_micros: 100,
    };
    let price_b = Price {
        input_micros: 3000,
        ..price_a.clone()
    };
    f.store.price(&binding.group_id, price_a.clone()).unwrap();
    other.price(&binding.group_id, price_b.clone()).unwrap();
    let mut request = f.call("history-0", "scan-a");
    request.participant_id = a.clone();
    request.group_id = binding.group_id.clone();
    request.target.as_mut().unwrap().budget_group_id = binding.group_id.clone();
    for i in 0..140 {
        request.id = format!("history-{i}");
        assert!(f.store.admit(&request, i * 1000).unwrap().admitted);
        f.store
            .settle(&request, &charge(&request.id, Some(100)))
            .unwrap();
    }
    for p in [&a, &b] {
        f.store.drain(&binding.group_id, 1, p).unwrap();
    }
    let frozen = f.store.freeze(&binding.group_id, 1).unwrap();
    // Exercise the versioned JSON transfer boundary as well as durable import.
    let frozen: AccountingSnapshot =
        serde_json::from_slice(&serde_json::to_vec(&frozen).unwrap()).unwrap();
    other.import(frozen.clone(), &b).unwrap();
    f.store.import(frozen, &b).unwrap();
    let original = f.store.snapshot(&binding.group_id).unwrap();
    assert_eq!(original.reservations.len(), 140);
    assert_eq!(original.omitted_reservations, 0);
    assert_eq!(bounded_snapshot(original).omitted_reservations, 12);
    assert_eq!(
        other
            .snapshot(&binding.group_id)
            .unwrap()
            .omitted_reservations,
        12
    );
    request.id = "new-owner-call".into();
    request.generation = 2;
    request.participant_id = b.clone();
    assert!(f.store.admit(&request, 140000).is_err());
    assert!(!other.admit(&request, 139999).unwrap().admitted);
    assert!(other.admit(&request, 140000).unwrap().admitted);
    other
        .settle(&request, &charge(&request.id, Some(100)))
        .unwrap();
    for p in [&a, &b] {
        other.drain(&binding.group_id, 2, p).unwrap();
    }
    let frozen = other.freeze(&binding.group_id, 2).unwrap();
    f.store.import(frozen.clone(), &a).unwrap();
    other.import(frozen, &a).unwrap();
    assert_eq!(
        f.store
            .snapshot(&binding.group_id)
            .unwrap()
            .reservations
            .len(),
        141
    );
    assert_eq!(
        f.store
            .snapshot(&binding.group_id)
            .unwrap()
            .omitted_reservations,
        0
    );
    assert_eq!(
        f.store.snapshot(&binding.group_id).unwrap().prices,
        vec![price_a]
    );
    assert_eq!(
        other.snapshot(&binding.group_id).unwrap().prices,
        vec![price_b]
    );
    assert_eq!(
        f.store.snapshot(&binding.group_id).unwrap().next_at_ms,
        141000
    );
    request.id = "returned-owner-call".into();
    request.generation = 3;
    request.participant_id = a;
    assert!(!f.store.admit(&request, 140999).unwrap().admitted);
    assert!(f.store.admit(&request, 141000).unwrap().admitted);
}

#[test]
fn external_allowance_usage_uses_aggregate_rate_with_different_scan_targets() {
    let f = Fixture::new();
    let mut fast = f.call("fast", "scan-fast");
    fast.target.as_mut().unwrap().amount = PacingAmount::Subscription {
        window_id: "primary".into(),
        basis_points: 600,
    };
    let mut slow = fast.clone();
    slow.id = "slow".into();
    slow.scan_id = "scan-slow".into();
    slow.target.as_mut().unwrap().amount = PacingAmount::Subscription {
        window_id: "primary".into(),
        basis_points: 60,
    };
    f.store
        .observe(
            &fast.group_id,
            1,
            Observation {
                window_id: "primary".into(),
                used_basis_points: 0,
                resets_at_ms: 100000,
                observed_at_ms: 0,
            },
            0,
        )
        .unwrap();
    assert!(f.store.admit(&fast, 0).unwrap().admitted);
    f.store.settle(&fast, &charge(&fast.id, None)).unwrap();
    assert!(f.store.admit(&slow, 1000).unwrap().admitted);
    f.store
        .observe(
            &fast.group_id,
            1,
            Observation {
                window_id: "primary".into(),
                used_basis_points: 100,
                resets_at_ms: 100000,
                observed_at_ms: 2000,
            },
            2000,
        )
        .unwrap();
    // 100 observed basis points minus 11 already reserved, at 10 points/second.
    assert_eq!(f.store.snapshot(&fast.group_id).unwrap().next_at_ms, 10900);
}

#[test]
fn reconnect_restores_missing_expired_and_future_scan_debt_once() {
    for prior_deadline in [None, Some(60_000), Some(160_000)] {
        for queued in [false, true] {
            let f = Fixture::new();
            let mut offline = f.call("offline", "scan-slow");
            offline.target.as_mut().unwrap().amount = PacingAmount::Currency {
                currency: "USD".into(),
                micros: 600,
            };
            if let Some(deadline) = prior_deadline {
                let mut previous = offline.clone();
                previous.id = "previous".into();
                assert!(
                    f.store
                        .admit(&previous, deadline - 60_000)
                        .unwrap()
                        .admitted
                );
                f.store
                    .settle(&previous, &charge(&previous.id, Some(600)))
                    .unwrap();
            }
            let fast = f.call("fast", "scan-fast");
            if f.store.admit(&fast, 100_000).unwrap().admitted {
                f.store.settle(&fast, &charge(&fast.id, Some(100))).unwrap();
            }
            if queued {
                assert!(!f.store.admit(&offline, 100_500).unwrap().admitted);
            }
            let aggregate_deadline = f.store.snapshot(&offline.group_id).unwrap().next_at_ms;
            f.store
                .reconcile(&offline, &charge(&offline.id, Some(600)), 101_000)
                .unwrap();
            let state = f.store.snapshot(&offline.group_id).unwrap();
            let scan_deadline = prior_deadline.unwrap_or(0).max(101_000) + 60_000;
            assert_eq!(state.scan_next["leader-a:scan-slow"], scan_deadline);
            assert_eq!(state.next_at_ms, aggregate_deadline.max(101_000) + 6000);
            let reopened = AccountingStore::open(HostDatabase::open(&f.root).unwrap()).unwrap();
            reopened
                .reconcile(&offline, &charge(&offline.id, Some(600)), 150_000)
                .unwrap();
            assert_eq!(
                serde_json::to_value(&state).unwrap(),
                serde_json::to_value(reopened.snapshot(&offline.group_id).unwrap()).unwrap()
            );
            // Clear a still-waiting fast ticket so it cannot affect the next
            // scan's fairness choice. Its captured aggregate rate is durable.
            reopened.cancel(&fast).unwrap();
            let mut next = offline.clone();
            next.id = "next".into();
            assert!(!reopened.admit(&next, scan_deadline - 1).unwrap().admitted);
            assert!(reopened.admit(&next, scan_deadline).unwrap().admitted);
        }
    }
}

#[test]
fn incompatible_reconciliation_is_atomic_and_preserves_group_admission() {
    for wrong_unit in [
        PacingAmount::Currency {
            currency: "EUR".into(),
            micros: 6000,
        },
        PacingAmount::Credits { micros: 6000 },
        PacingAmount::Subscription {
            window_id: "example-window".into(),
            basis_points: 6000,
        },
    ] {
        let f = Fixture::new();
        let first = f.call("first", "scan-first");
        assert!(f.store.admit(&first, 100_000).unwrap().admitted);
        f.store
            .settle(&first, &charge(&first.id, Some(100)))
            .unwrap();
        let before = serde_json::to_value(f.store.snapshot(&first.group_id).unwrap()).unwrap();
        let mut offline = f.call("offline", "scan-offline");
        offline.target.as_mut().unwrap().amount = wrong_unit;
        assert!(f.store.admit(&offline, 101_000).is_err());
        for _ in 0..2 {
            assert!(
                f.store
                    .reconcile(&offline, &charge(&offline.id, Some(600)), 101_000)
                    .is_err()
            );
            assert_eq!(
                before,
                serde_json::to_value(f.store.snapshot(&first.group_id).unwrap()).unwrap()
            );
        }
        let reopened = AccountingStore::open(HostDatabase::open(&f.root).unwrap()).unwrap();
        assert_eq!(
            before,
            serde_json::to_value(reopened.snapshot(&first.group_id).unwrap()).unwrap()
        );
        // Provider cleanup may cancel a rejected admission. Its tombstone must
        // still fence delayed delivery without becoming the group's unit.
        let mut canceled = offline.clone();
        canceled.id = "rejected-cleanup".into();
        reopened.cancel(&canceled).unwrap();
        assert!(reopened.admit(&canceled, 101_000).is_err());
        assert!(
            reopened
                .admit(&f.call("next", "scan-first"), 101_000)
                .unwrap()
                .admitted
        );
        // Correcting the rejected request can reuse its identity: no partial
        // reservation or charge was left behind by validation failure.
        offline.target = first.target.clone();
        reopened
            .reconcile(&offline, &charge(&offline.id, Some(600)), 102_000)
            .unwrap();
    }
}
