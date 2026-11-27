use beale_contracts::swarm_allocation::*;
use beale_storage::{HostDatabase, fleet_allocation::AllocationStore};
use std::collections::{BTreeMap, BTreeSet};

struct Fixture {
    root: std::path::PathBuf,
    store: Option<AllocationStore>,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("beale-allocation-example-{}", uuid::Uuid::new_v4()));
        let store = AllocationStore::open(HostDatabase::open(&root).unwrap()).unwrap();
        Self {
            root,
            store: Some(store),
        }
    }
    fn store(&self) -> &AllocationStore {
        self.store.as_ref().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.store.take();
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}
fn capacity() -> Capacity {
    Capacity {
        domain: "docker-example".into(),
        observed_at_ms: 1000,
        available_memory: RAM_FLOOR + DEFAULT_MEMORY + 1,
        storage: BTreeMap::from([("disk-example".into(), STORAGE_RESERVE + DEFAULT_STORAGE * 8)]),
        measured_memory: BTreeMap::new(),
        measured_storage: BTreeSet::new(),
    }
}

#[test]
fn ended_scan_cannot_admit_new_environment_until_a_newer_reopen() {
    let f = Fixture::new();
    let c = capacity();
    let mut lifecycle = ScanLifecycle {
        leader: "leader-example".into(),
        scan: "scan-example".into(),
        revision: 1,
        ended_at_ms: Some(1000),
        retention_seconds: 604800,
    };
    f.store().scan_lifecycle(&lifecycle, 1000).unwrap();
    f.store().enqueue(request("ended"), &c, 1000).unwrap();
    assert!(f.store().admit_next(&c, 1000).unwrap().is_none());
    assert_eq!(
        f.store().allocation("ended").unwrap().phase,
        AllocationPhase::Blocked
    );
    assert!(f.store().environments().unwrap().is_empty());
    lifecycle.revision += 1;
    lifecycle.ended_at_ms = None;
    f.store().scan_lifecycle(&lifecycle, 1000).unwrap();
    f.store().enqueue(request("reopened"), &c, 1000).unwrap();
    assert_eq!(
        f.store().admit_next(&c, 1000).unwrap().unwrap().request.id,
        "reopened"
    );
}

#[test]
fn cancellation_before_lost_enqueue_prevents_a_later_launch() {
    let f = Fixture::new();
    f.store().cancel_queued("late").unwrap();
    let row = f
        .store()
        .enqueue(request("late"), &capacity(), 1000)
        .unwrap();
    assert_eq!(row.phase, AllocationPhase::Canceled);
    assert!(f.store().admit_next(&capacity(), 1000).unwrap().is_none());
    assert!(f.store().environments().unwrap().is_empty());
}

#[test]
fn concurrent_keep_or_resume_and_cleanup_have_one_winner() {
    for action in [
        RetentionAction::Keep { value: true },
        RetentionAction::Reopen,
    ] {
        let f = Fixture::new();
        let a = admitted(&f, "a");
        f.store().confirm_stopped("a", true).unwrap();
        let e = f.store().environment(&a.request.instance).unwrap();
        let e = f
            .store()
            .retention(
                &e.instance,
                e.generation,
                RetentionAction::End {
                    ended_at_ms: 1000,
                    retention_seconds: 0,
                },
                1000,
            )
            .unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let tasks: Vec<_> = [action, RetentionAction::ClaimDelete { now: false }]
            .into_iter()
            .map(|action| {
                let s = AllocationStore::open(HostDatabase::open(&f.root).unwrap()).unwrap();
                let e = e.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    s.retention(&e.instance, e.generation, action, 1000).is_ok()
                })
            })
            .collect();
        assert_eq!(
            tasks
                .into_iter()
                .map(|t| usize::from(t.join().unwrap()))
                .sum::<usize>(),
            1
        );
        let result = f.store().environment(&e.instance).unwrap();
        assert!(
            result.deletion == DeletionStep::Claimed || result.keep || result.deadline_ms.is_none()
        );
    }
}

#[test]
fn pending_output_discard_requires_separate_exact_unexpired_confirmation() {
    let f = Fixture::new();
    let a = admitted(&f, "a");
    let e = f.store().environment(&a.request.instance).unwrap();
    f.store()
        .retention(
            &e.instance,
            e.generation,
            RetentionAction::Pending {
                operation: "upload-example".into(),
            },
            1000,
        )
        .unwrap();
    f.store().confirm_stopped("a", false).unwrap();
    let e = f.store().environment(&e.instance).unwrap();
    assert!(
        f.store()
            .retention(
                &e.instance,
                e.generation,
                RetentionAction::ClaimDelete { now: true },
                1000
            )
            .is_err()
    );
    let preview = f
        .store()
        .preview_discard(&e.instance, e.generation, 1000)
        .unwrap();
    assert_eq!(
        preview.operations,
        BTreeSet::from(["upload-example".into()])
    );
    let mut altered = preview.clone();
    altered.operations.clear();
    assert!(f.store().confirm_discard(&altered, 1000).is_err());
    assert!(f.store().confirm_discard(&preview, 301001).is_err());
    let done = f.store().confirm_discard(&preview, 2000).unwrap();
    assert!(
        done.pending_outputs.is_empty() && done.references.is_empty() && done.checkpoint_confirmed
    );
    assert_eq!(f.store().confirm_discard(&preview, 999999).unwrap(), done);
    f.store()
        .retention(
            &done.instance,
            done.generation,
            RetentionAction::ClaimDelete { now: true },
            2000,
        )
        .unwrap();
}

#[test]
fn canonical_storage_reserves_shared_disk_without_granting_execution() {
    let f = Fixture::new();
    let mut c = capacity();
    c.domain = "canonical-storage".into();
    c.available_memory = u64::MAX;
    let mut r = request("central");
    r.domain = c.domain.clone();
    r.memory = 1;
    f.store().enqueue(r.clone(), &c, 1000).unwrap();
    f.store().admit_next(&c, 1000).unwrap().unwrap();
    let mut native = capacity();
    native
        .storage
        .insert("disk-example".into(), STORAGE_RESERVE + DEFAULT_STORAGE);
    assert!(matches!(
        f.store()
            .feasibility(&request("native"), &native, 1000)
            .unwrap(),
        Feasibility::Denied { .. }
    ));
    f.store().release_canonical_storage("central").unwrap();
    assert_eq!(
        f.store()
            .feasibility(&request("native"), &native, 1000)
            .unwrap(),
        Feasibility::Ready
    );
    assert!(f.store().environment(&r.instance).is_err());
    f.store().release_canonical_storage("central").unwrap();
}

#[test]
fn measured_private_storage_still_enforces_the_reserve() {
    let f = Fixture::new();
    let r = request("a");
    let mut c = capacity();
    c.measured_storage.insert(r.storage[0].key.clone());
    c.storage.insert("disk-example".into(), STORAGE_RESERVE - 1);
    assert!(matches!(
        f.store().feasibility(&r, &c, 1000).unwrap(),
        Feasibility::Denied { .. }
    ));
}

#[test]
fn accepted_work_survives_competing_scan_and_cancel_consumes_no_lease() {
    let f = Fixture::new();
    let parent = admitted(&f, "parent");
    let mut child = request("child");
    child.branch = "parent".into();
    child.release_parent = Some("parent".into());
    let mut c = capacity();
    c.available_memory = RAM_FLOOR + 1;
    c.measured_memory.insert("parent".into(), DEFAULT_MEMORY);
    assert!(matches!(
        f.store().feasibility(&child, &c, 1000).unwrap(),
        Feasibility::ReleaseParent { .. }
    ));
    f.store().confirm_stopped("parent", true).unwrap();
    c = capacity();
    let mut other = request("other");
    other.scan = "competing-scan".into();
    f.store().enqueue(other, &c, 1000).unwrap();
    f.store().admit_next(&c, 1000).unwrap().unwrap();
    let queued = f.store().enqueue(child, &c, 1000).unwrap();
    assert_eq!(queued.phase, AllocationPhase::Queued);
    assert!(f.store().admit_next(&c, 1000).unwrap().is_none());
    f.store().confirm_stopped("other", true).unwrap();
    assert_eq!(
        f.store().admit_next(&c, 1000).unwrap().unwrap().request.id,
        "child"
    );
    assert!(f.store().admit_next(&c, 1000).unwrap().is_none());
    let queued = f.store().enqueue(request("cancelled"), &c, 1000).unwrap();
    f.store().cancel_queued(&queued.request.id).unwrap();
    assert_eq!(
        f.store().allocation("cancelled").unwrap().phase,
        AllocationPhase::Canceled
    );
    assert!(
        f.store()
            .environment(&parent.request.instance)
            .unwrap()
            .checkpoint_confirmed
    );
}

#[test]
fn failed_parent_checkpoint_blocks_even_when_ram_has_become_free() {
    let f = Fixture::new();
    admitted(&f, "parent");
    let mut child = request("child");
    child.branch = "parent".into();
    child.release_parent = Some("parent".into());
    let c = capacity();
    f.store().enqueue(child, &c, 1000).unwrap();
    f.store().confirm_stopped("parent", false).unwrap();
    assert!(f.store().admit_next(&c, 1000).unwrap().is_none());
    let mut later = c.clone();
    later.observed_at_ms = 301001;
    assert!(f.store().admit_next(&later, 301001).unwrap().is_none());
    assert_eq!(
        f.store().allocation("child").unwrap().phase,
        AllocationPhase::Blocked
    );
}

#[test]
fn fair_scans_and_result_wakeups_progress_without_duplicate_admission() {
    let f = Fixture::new();
    let c = capacity();
    for (id, scan, branch, wake) in [
        ("a", "scan-a", "branch-a", false),
        ("b", "scan-b", "branch-b", false),
        ("wake", "scan-a", "branch-a", true),
        ("worker", "scan-a", "branch-new", false),
    ] {
        let mut r = request(id);
        r.scan = scan.into();
        r.branch = branch.into();
        r.processing_result = wake;
        f.store().enqueue(r, &c, 1000).unwrap();
    }
    let mut order = vec![];
    for _ in 0..4 {
        let row = f.store().admit_next(&c, 1000).unwrap().unwrap();
        order.push(row.request.id.clone());
        assert!(f.store().admit_next(&c, 1000).unwrap().is_none());
        f.store().confirm_stopped(&row.request.id, true).unwrap();
    }
    assert_eq!(order[0], "wake");
    assert_eq!(order[1], "b");
    assert_eq!(order.iter().collect::<BTreeSet<_>>().len(), 4);
}

#[test]
fn full_retry_specification_and_instance_choice_are_durable() {
    let f = Fixture::new();
    let r = request("identity");
    let s = AllocationSpec {
        id: r.id.clone(),
        leader: r.leader.clone(),
        scan: r.scan.clone(),
        agent: r.agent.clone(),
        parent: r.branch.clone(),
        configuration: r.configuration.clone(),
        input_bytes: 100,
        storage_reserve: STORAGE_RESERVE,
        processing_result: false,
        restore: None,
    };
    assert_eq!(f.store().bind_spec(&s, &r).unwrap(), r);
    let mut proposed = r.clone();
    proposed.instance = uuid::Uuid::new_v4().to_string();
    assert_eq!(f.store().bind_spec(&s, &proposed).unwrap(), r);
    let mut changed = s.clone();
    changed.input_bytes += 1;
    assert!(f.store().bound_spec(&changed).is_err());
    changed = s.clone();
    changed.parent = "different-parent".into();
    assert!(f.store().bind_spec(&changed, &r).is_err());
}

#[test]
fn lifecycle_reordering_cannot_reend_reopened_scan_or_revive_deleted_state() {
    let f = Fixture::new();
    let row = admitted(&f, "a");
    f.store().confirm_stopped("a", true).unwrap();
    let end = ScanLifecycle {
        leader: row.request.leader.clone(),
        scan: row.request.scan.clone(),
        revision: 2,
        ended_at_ms: Some(1000),
        retention_seconds: 7 * 86400,
    };
    f.store().scan_lifecycle(&end, 1000).unwrap();
    let mut reopen = end.clone();
    reopen.revision = 3;
    reopen.ended_at_ms = None;
    f.store().scan_lifecycle(&reopen, 1000).unwrap();
    f.store().scan_lifecycle(&end, 1000).unwrap();
    let env = f.store().environment(&row.request.instance).unwrap();
    assert_eq!(env.deadline_ms, None);
    let claimed = f
        .store()
        .retention(
            &env.instance,
            env.generation,
            RetentionAction::ClaimDelete { now: true },
            1000,
        )
        .unwrap();
    reopen.revision = 4;
    assert!(f.store().scan_lifecycle(&reopen, 1000).is_err());
    assert_eq!(f.store().environment(&env.instance).unwrap(), claimed);
}

#[test]
fn failed_publication_and_execution_protections_cannot_be_cleared_by_generic_release() {
    let f = Fixture::new();
    let row = admitted(&f, "a");
    let env = f.store().environment(&row.request.instance).unwrap();
    assert!(
        f.store()
            .retention(
                &env.instance,
                env.generation,
                RetentionAction::Release {
                    reference: "a".into()
                },
                1000
            )
            .is_err()
    );
    let env = f
        .store()
        .retention(
            &env.instance,
            env.generation,
            RetentionAction::Pending {
                operation: "publication-example".into(),
            },
            1000,
        )
        .unwrap();
    f.store().confirm_stopped("a", true).unwrap();
    let env = f.store().environment(&env.instance).unwrap();
    assert!(!env.checkpoint_confirmed);
    assert!(
        f.store()
            .retention(
                &env.instance,
                env.generation,
                RetentionAction::ClaimDelete { now: true },
                1000
            )
            .is_err()
    );
}
fn request(id: &str) -> AllocationRequest {
    AllocationRequest {
        version: 1,
        id: id.into(),
        leader: "leader-example".into(),
        scan: "scan-example".into(),
        agent: id.into(),
        branch: id.into(),
        domain: "docker-example".into(),
        configuration: "image-example".into(),
        configuration_revision: 1,
        instance: uuid::Uuid::new_v4().to_string(),
        memory: DEFAULT_MEMORY,
        storage: vec![StorageDemand {
            pool: "disk-example".into(),
            key: format!("private-{id}"),
            bytes: DEFAULT_STORAGE,
        }],
        storage_reserve: STORAGE_RESERVE,
        release_parent: None,
        processing_result: false,
    }
}
fn admitted(f: &Fixture, id: &str) -> Allocation {
    let c = capacity();
    f.store().enqueue(request(id), &c, 1000).unwrap();
    f.store().admit_next(&c, 1000).unwrap().unwrap()
}

#[test]
fn concurrent_owners_cannot_double_reserve_capacity() {
    let f = Fixture::new();
    let c = capacity();
    f.store().enqueue(request("a"), &c, 1000).unwrap();
    f.store().enqueue(request("b"), &c, 1000).unwrap();
    let stores = [
        AllocationStore::open(HostDatabase::open(&f.root).unwrap()).unwrap(),
        AllocationStore::open(HostDatabase::open(&f.root).unwrap()).unwrap(),
    ];
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let tasks: Vec<_> = stores
        .into_iter()
        .map(|s| {
            let b = barrier.clone();
            let c = c.clone();
            std::thread::spawn(move || {
                b.wait();
                s.admit_next(&c, 1000).unwrap()
            })
        })
        .collect();
    assert_eq!(
        tasks.into_iter().filter_map(|t| t.join().unwrap()).count(),
        1
    );
    assert_eq!(
        f.store()
            .allocations()
            .unwrap()
            .iter()
            .filter(|a| a.phase == AllocationPhase::Reserved)
            .count(),
        1
    );
}
#[test]
fn effective_domain_floor_estimates_and_reflected_usage() {
    let f = Fixture::new();
    let mut c = capacity();
    let a = request("a");
    c.available_memory = RAM_FLOOR + DEFAULT_MEMORY;
    assert!(matches!(
        f.store().feasibility(&a, &c, 1000).unwrap(),
        Feasibility::Denied { .. }
    ));
    c.available_memory += 1;
    f.store().enqueue(a, &c, 1000).unwrap();
    f.store().admit_next(&c, 1000).unwrap().unwrap();
    c.available_memory = RAM_FLOOR + DEFAULT_MEMORY + 1;
    assert!(matches!(
        f.store().feasibility(&request("b"), &c, 1000).unwrap(),
        Feasibility::Denied { .. }
    ));
    c.measured_memory.insert("a".into(), DEFAULT_MEMORY);
    assert_eq!(
        f.store().feasibility(&request("b"), &c, 1000).unwrap(),
        Feasibility::Ready
    );
    c.domain = "different-vm-example".into();
    assert!(f.store().feasibility(&request("b"), &c, 1000).is_err());
    c = capacity();
    assert!(f.store().feasibility(&request("b"), &c, 31_001).is_err());
    c.storage.clear();
    assert!(f.store().feasibility(&request("b"), &c, 1000).is_err());
}
#[test]
fn parent_release_is_feasibility_only_and_never_reclaims_retained_disk() {
    let f = Fixture::new();
    let a = admitted(&f, "parent");
    f.store().started("parent").unwrap();
    let mut c = capacity();
    c.available_memory = RAM_FLOOR + 1;
    c.measured_memory.insert("parent".into(), DEFAULT_MEMORY);
    let mut child = request("child");
    child.branch = "parent".into();
    child.release_parent = Some("parent".into());
    assert!(matches!(
        f.store().feasibility(&child, &c, 1000).unwrap(),
        Feasibility::ReleaseParent { .. }
    ));
    let queued = f.store().enqueue(child.clone(), &c, 1000).unwrap();
    assert_eq!(queued.phase, AllocationPhase::Queued);
    assert!(f.store().admit_next(&c, 1000).unwrap().is_none());
    // Failed checkpoint does not manufacture free memory or a launch permit.
    f.store().confirm_stopped("parent", false).unwrap();
    assert!(f.store().admit_next(&c, 1000).unwrap().is_none());
    c.available_memory += DEFAULT_MEMORY;
    c.measured_memory.clear();
    f.store().confirm_stopped("parent", true).unwrap();
    assert_eq!(
        f.store().admit_next(&c, 1000).unwrap().unwrap().request.id,
        "child"
    );
    assert!(
        f.store()
            .environment(&a.request.instance)
            .unwrap()
            .checkpoint_confirmed
    );
    let mut poor = capacity();
    poor.storage
        .insert("disk-example".into(), STORAGE_RESERVE + DEFAULT_STORAGE);
    assert!(matches!(
        f.store()
            .feasibility(&request("another"), &poor, 1000)
            .unwrap(),
        Feasibility::Denied { .. }
    ));
}
#[test]
fn pending_shared_image_layers_are_charged_once_and_private_inputs_are_not_free() {
    let f = Fixture::new();
    let mut c = capacity();
    c.available_memory = RAM_FLOOR + DEFAULT_MEMORY * 4;
    let mut a = request("a");
    a.storage.push(StorageDemand {
        pool: "disk-example".into(),
        key: "image-layer-example".into(),
        bytes: DEFAULT_STORAGE,
    });
    f.store().enqueue(a, &c, 1000).unwrap();
    f.store().admit_next(&c, 1000).unwrap().unwrap();
    let mut b = request("b");
    b.storage.push(StorageDemand {
        pool: "disk-example".into(),
        key: "image-layer-example".into(),
        bytes: DEFAULT_STORAGE,
    });
    c.storage
        .insert("disk-example".into(), STORAGE_RESERVE + DEFAULT_STORAGE * 3);
    assert_eq!(
        f.store().feasibility(&b, &c, 1000).unwrap(),
        Feasibility::Ready
    );
    c.storage.insert(
        "disk-example".into(),
        STORAGE_RESERVE + DEFAULT_STORAGE * 3 - 1,
    );
    assert!(matches!(
        f.store().feasibility(&b, &c, 1000).unwrap(),
        Feasibility::Denied { .. }
    ));
    c.measured_storage.insert("image-layer-example".into());
    assert_eq!(
        f.store().feasibility(&b, &c, 1000).unwrap(),
        Feasibility::Ready
    );
}
#[test]
fn abandoned_reservations_require_backend_reconciliation_and_waits_are_bounded() {
    let f = Fixture::new();
    let a = admitted(&f, "a");
    let mut c = capacity();
    c.observed_at_ms = 400_000;
    assert!(f.store().admit_next(&c, 400_000).unwrap().is_none());
    assert_eq!(
        f.store().allocation("a").unwrap().phase,
        AllocationPhase::Reserved
    );
    f.store().started("a").unwrap();
    assert!(f.store().cancel_queued("a").is_err());
    f.store().confirm_stopped("a", true).unwrap();
    f.store().enqueue(request("b"), &c, 400_000).unwrap();
    c.observed_at_ms = 800_000;
    f.store().admit_next(&c, 800_000).unwrap();
    assert_eq!(
        f.store().allocation("b").unwrap().phase,
        AllocationPhase::Blocked
    );
    assert!(
        f.store()
            .environment(&a.request.instance)
            .unwrap()
            .termination_confirmed
    );
}
#[test]
fn retention_deadlines_keep_reopen_and_pending_outputs_are_protected() {
    let f = Fixture::new();
    let a = admitted(&f, "a");
    f.store().confirm_stopped("a", true).unwrap();
    let id = &a.request.instance;
    let change = |action, now| {
        let e = f.store().environment(id).unwrap();
        f.store().retention(id, e.generation, action, now)
    };
    assert!(change(RetentionAction::ClaimDelete { now: false }, 9_000_000).is_err());
    change(
        RetentionAction::End {
            ended_at_ms: 1000,
            retention_seconds: 7 * 86_400,
        },
        1000,
    )
    .unwrap();
    assert!(change(RetentionAction::ClaimDelete { now: false }, 604_800_999).is_err());
    change(RetentionAction::Keep { value: true }, 604_801_000).unwrap();
    assert!(change(RetentionAction::ClaimDelete { now: false }, 604_801_000).is_err());
    change(RetentionAction::Reopen, 604_801_000).unwrap();
    assert_eq!(f.store().environment(id).unwrap().deadline_ms, None);
    change(
        RetentionAction::Pending {
            operation: "upload-example".into(),
        },
        604_801_000,
    )
    .unwrap();
    assert!(change(RetentionAction::ClaimDelete { now: true }, 604_801_000).is_err());
    change(
        RetentionAction::Acknowledge {
            operation: "upload-example".into(),
        },
        604_801_000,
    )
    .unwrap();
    assert!(change(RetentionAction::ClaimDelete { now: true }, 604_801_000).is_err());
    change(RetentionAction::Checkpoint, 604_801_000).unwrap();
    change(RetentionAction::ClaimDelete { now: true }, 604_801_000).unwrap();
    assert!(change(RetentionAction::Reopen, 604_801_000).is_err());
}
#[test]
fn cleanup_and_keep_race_has_one_winner_and_deletion_resumes_exact_generation() {
    let f = Fixture::new();
    let a = admitted(&f, "a");
    f.store().confirm_stopped("a", true).unwrap();
    let e = f.store().environment(&a.request.instance).unwrap();
    let e = f
        .store()
        .retention(
            &e.instance,
            e.generation,
            RetentionAction::End {
                ended_at_ms: 1000,
                retention_seconds: 0,
            },
            1000,
        )
        .unwrap();
    let claim = f
        .store()
        .retention(
            &e.instance,
            e.generation,
            RetentionAction::ClaimDelete { now: false },
            1000,
        )
        .unwrap();
    assert!(
        f.store()
            .retention(
                &e.instance,
                e.generation,
                RetentionAction::Keep { value: true },
                1000
            )
            .is_err()
    );
    f.store()
        .deletion_step(
            &e.instance,
            claim.generation,
            DeletionStep::ContainerRemoved,
        )
        .unwrap();
    let reopened = AllocationStore::open(HostDatabase::open(&f.root).unwrap()).unwrap();
    assert_eq!(
        reopened.environment(&e.instance).unwrap().deletion,
        DeletionStep::ContainerRemoved
    );
    assert!(
        reopened
            .deletion_step(
                &e.instance,
                claim.generation - 1,
                DeletionStep::VolumeRemoved
            )
            .is_err()
    );
    reopened
        .deletion_step(&e.instance, claim.generation, DeletionStep::VolumeRemoved)
        .unwrap();
    reopened
        .deletion_step(&e.instance, claim.generation, DeletionStep::Deleted)
        .unwrap();
    reopened
        .deletion_step(&e.instance, claim.generation, DeletionStep::Deleted)
        .unwrap();
}

#[test]
fn old_stop_receipt_cannot_modify_reused_environment() {
    let f = Fixture::new();
    let old = admitted(&f, "old-example");
    f.store().confirm_stopped("old-example", true).unwrap();
    let mut next = old.request.clone();
    next.id = "new-example".into();
    f.store().enqueue(next, &capacity(), 1000).unwrap();
    f.store().admit_next(&capacity(), 1000).unwrap().unwrap();
    let before = f.store().environment(&old.request.instance).unwrap();
    f.store().confirm_stopped("old-example", true).unwrap();
    assert_eq!(
        f.store().environment(&old.request.instance).unwrap(),
        before
    );
    f.store().started("new-example").unwrap();
    let before = f.store().environment(&old.request.instance).unwrap();
    f.store().confirm_stopped("old-example", true).unwrap();
    assert_eq!(
        f.store().environment(&old.request.instance).unwrap(),
        before
    );
}
#[test]
fn transfer_renewal_cannot_revive_expired_or_started_reservations() {
    let f = Fixture::new();
    admitted(&f, "transfer-example");
    f.store().renew_transfer("transfer-example", 2000).unwrap();
    assert_eq!(
        f.store()
            .allocation("transfer-example")
            .unwrap()
            .reservation_deadline_ms,
        662_000
    );
    assert!(
        f.store()
            .renew_transfer("transfer-example", 663_000)
            .is_err()
    );
    f.store().started("transfer-example").unwrap();
    assert!(f.store().renew_transfer("transfer-example", 3000).is_err());
}
