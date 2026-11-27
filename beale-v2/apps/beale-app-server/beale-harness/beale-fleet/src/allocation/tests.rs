use super::*;
use crate::docker::*;
use beale_contracts::{
    swarm_container::*, swarm_host::ExecutionTarget, swarm_worker::ExecutionIdentity,
};
use std::collections::{BTreeMap, BTreeSet};
struct Fixed;
impl CapacitySource for Fixed {
    fn measure<'a>(&'a self, _: &'a DockerEnvironments, _: &'a [Allocation]) -> CapacityFuture<'a> {
        Box::pin(async {
            Ok(Capacity {
                domain: "docker".into(),
                observed_at_ms: Allocator::now(),
                available_memory: RAM_FLOOR + DEFAULT_MEMORY + 1,
                storage: BTreeMap::from([("disk-example".into(), 128 * 1024 * 1024 * 1024)]),
                measured_memory: BTreeMap::new(),
                measured_storage: BTreeSet::new(),
            })
        })
    }
}
fn allocator(root: &std::path::Path) -> Allocator {
    let db = HostDatabase::open(root).unwrap();
    let docker = Docker::new(
        std::env::var_os("BEALE_DOCKER_CLI")
            .map(PathBuf::from)
            .unwrap_or_else(|| "docker".into()),
    );
    let env = DockerEnvironments::new(
        docker,
        DockerStore::open(db.clone()).unwrap(),
        root.into(),
        std::env::var_os("BEALE_CONTAINER_RUNTIME").map(PathBuf::from),
    )
    .unwrap();
    env.store
        .save_configuration(ImageConfiguration {
            version: 1,
            id: "image-example".into(),
            revision: 1,
            image: "alpine:3.22".into(),
            architecture: LinuxArchitecture::Amd64,
            options: ContainerOptions::default(),
        })
        .unwrap();
    Allocator::new(db, env, Arc::new(Fixed)).unwrap()
}
fn spec(id: &str) -> AllocationSpec {
    AllocationSpec {
        id: id.into(),
        leader: "leader-example".into(),
        scan: "scan-example".into(),
        agent: "agent-example".into(),
        parent: "parent-example".into(),
        configuration: "image-example".into(),
        input_bytes: 100,
        storage_reserve: STORAGE_RESERVE,
        processing_result: false,
        restore: None,
    }
}
async fn enqueue(a: &Allocator, s: AllocationSpec) -> Allocation {
    let HostAllocationReply::Allocation { allocation } =
        http::handle(a, HostAllocationRequest::Enqueue { spec: s })
            .await
            .unwrap()
    else {
        panic!("allocation not queued")
    };
    allocation
}
async fn command(a: &Allocator, h: &ContainerHandle, script: &str) -> String {
    let out = a
        .environments
        .docker
        .command()
        .args([
            "exec",
            "--",
            &h.id,
            &format!("{}/bash", h.runtime_directory),
            "--noprofile",
            "--norc",
            "-c",
            script,
        ])
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[tokio::test]
#[ignore = "requires local Docker execution-domain access"]
async fn docker_measures_execution_domain_and_backing_storage() {
    let root =
        std::env::temp_dir().join(format!("beale-capacity-example-{}", uuid::Uuid::new_v4()));
    let a = allocator(&root);
    let c = capacity::measure(&root, &a.environments, &[])
        .await
        .unwrap();
    c.validate(Allocator::now()).unwrap();
    assert!(c.available_memory > 0);
    assert!(!c.storage.is_empty());
    drop(a);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
#[ignore = "requires Docker and a portable worker runtime bundle"]
async fn docker_retained_restore_checkpoint_protection_and_claimed_cleanup() {
    let root = std::env::temp_dir().join(format!("beale-restore-example-{}", uuid::Uuid::new_v4()));
    let source = allocator(&root.join("source"));
    let destination = allocator(&root.join("destination"));
    // Revision drift before creation must not poison this agent's later retry.
    enqueue(&source, spec("stale-revision")).await;
    let mut changed = source
        .environments
        .store
        .configuration("image-example")
        .unwrap()
        .configuration;
    changed.revision += 1;
    source
        .environments
        .store
        .save_configuration(changed)
        .unwrap();
    assert!(source.prepare_next().await.is_err());
    http::handle(
        &source,
        HostAllocationRequest::Cancel {
            id: "stale-revision".into(),
        },
    )
    .await
    .unwrap();
    enqueue(&source, spec("first")).await;
    let allocated = source.prepare_next().await.unwrap().unwrap();
    let mut instance = source
        .environments
        .store
        .instance(&allocated.request.instance)
        .unwrap();
    let handle = instance.handle.clone().unwrap();
    let target = ExecutionTarget {
        leader_id: "leader-example".into(),
        host_id: source.environments.store.host().into(),
        configuration_id: "image-example".into(),
        configuration_revision: allocated.request.configuration_revision,
        instance_id: instance.id.to_string(),
        identity: ExecutionIdentity {
            workspace_id: "workspace-example".into(),
            scan_id: "scan-example".into(),
            agent_id: "agent-example".into(),
            attempt_id: "attempt-example".into(),
            generation: 1,
        },
    };
    instance.target = Some(target.clone());
    source.environments.store.save_instance(&instance).unwrap();
    source.store.bind_execution("first", &target).unwrap();
    source.store.started("first").unwrap();
    source.environments.docker.start(&handle).await.unwrap();
    command(&source,&handle,&format!("mkdir -p /beale-workspace/workspace {}/state/plugins/example; printf 'private edit' > /beale-workspace/workspace/edit.txt; printf 'installed tooling' > /beale-installed-example; printf 'prepared plugin' > {}/state/plugins/example/tool.txt",handle.runtime_directory,handle.runtime_directory)).await;
    assert!(source.environments.docker.stop(&handle).await.unwrap());
    // Maintenance may still hold the pre-launch snapshot. It must reload the
    // bound execution under the instance lock and retain checkpoint protection.
    source.reconcile(&allocated).await.unwrap();
    let reconciled = source.store.environment(&target.instance_id).unwrap();
    assert!(!reconciled.checkpoint_confirmed);
    assert!(reconciled.references.contains("first"));
    // A failed checkpoint write must retain the same publication dependency;
    // removing the synthetic obstruction permits the original operation retry.
    let incomplete = EnvironmentBundle {
        version: 1,
        target: target.clone(),
        configuration: instance.configuration.clone(),
        image_digest: None,
        archives: vec![],
    };
    let checkpoint_root = source
        .environments
        .root()
        .join("allocation-archives")
        .join(archives::bundle_id(&incomplete).unwrap());
    std::fs::create_dir_all(checkpoint_root.join("workspace.part")).unwrap();
    assert!(source.capture(&target, false).await.is_err());
    let failed = source.store.environment(&target.instance_id).unwrap();
    assert!(!failed.checkpoint_confirmed && !failed.pending_outputs.is_empty());
    assert!(!checkpoint_root.join("manifest.json").exists());
    std::fs::remove_dir(checkpoint_root.join("workspace.part")).unwrap();
    let checkpoint = source.capture(&target, false).await.unwrap();
    assert_eq!(source.capture(&target, false).await.unwrap(), checkpoint);
    let protected = source.store.environment(&target.instance_id).unwrap();
    assert!(!protected.pending_outputs.is_empty());
    assert!(
        source
            .store
            .retention(
                &target.instance_id,
                protected.generation,
                RetentionAction::ClaimDelete { now: true },
                Allocator::now()
            )
            .is_err()
    );
    assert!(
        source
            .accept_checkpoint(&target, "incorrect-digest")
            .await
            .is_err()
    );
    source
        .accept_checkpoint(&target, &archives::bundle_digest(&checkpoint).unwrap())
        .await
        .unwrap();
    // A delayed acknowledgement of the old turn cannot release the next
    // reservation or change its lifecycle, even before the next launch binds.
    enqueue(&source, spec("next-reservation")).await;
    let next = source.prepare_next().await.unwrap().unwrap();
    let before = source.store.environment(&target.instance_id).unwrap();
    source
        .accept_checkpoint(&target, &archives::bundle_digest(&checkpoint).unwrap())
        .await
        .unwrap();
    assert_eq!(
        source.store.allocation("next-reservation").unwrap().phase,
        AllocationPhase::Reserved
    );
    assert_eq!(
        source.store.environment(&target.instance_id).unwrap(),
        before
    );
    assert!(before.references.contains("next-reservation"));
    source.reconcile(&next).await.unwrap();
    source.store.cancel_queued("next-reservation").unwrap();
    let mut full_intent = incomplete.clone();
    full_intent.image_digest = Some("pending".into());
    let export_root = source
        .environments
        .root()
        .join("allocation-archives")
        .join(archives::bundle_id(&full_intent).unwrap());
    std::fs::create_dir_all(export_root.join("workspace.part")).unwrap();
    assert!(source.capture(&target, true).await.is_err());
    assert!(
        source
            .store
            .archive_ids(&target.instance_id)
            .unwrap()
            .contains(&archives::bundle_id(&full_intent).unwrap())
    );
    assert!(
        source
            .store
            .environment(&target.instance_id)
            .unwrap()
            .references
            .contains("restore-attempt-example")
    );
    std::fs::remove_dir(export_root.join("workspace.part")).unwrap();
    let bundle = source.capture(&target, true).await.unwrap();
    assert_eq!(source.capture(&target, true).await.unwrap(), bundle);
    let protected = source.store.environment(&target.instance_id).unwrap();
    assert!(
        source
            .store
            .retention(
                &target.instance_id,
                protected.generation,
                RetentionAction::ClaimDelete { now: true },
                Allocator::now()
            )
            .is_err()
    );
    let mut s = spec("restored");
    s.restore = Some(bundle.clone());
    let mut incompatible = s.clone();
    incompatible
        .restore
        .as_mut()
        .unwrap()
        .configuration
        .architecture = LinuxArchitecture::Arm64;
    assert!(
        http::handle(
            &destination,
            HostAllocationRequest::Feasibility { spec: incompatible }
        )
        .await
        .is_err()
    );
    enqueue(&destination, s).await;
    let reserved = destination.prepare_next().await.unwrap().unwrap();
    assert!(!reserved.prepared);
    destination.begin_restore("restored", &bundle).unwrap();
    assert!(destination.finish_restore("restored").await.is_err());
    for entry in &bundle.archives {
        let mut offset = 0;
        while offset < entry.bytes {
            let bytes = source
                .read_archive(&archives::bundle_id(&bundle).unwrap(), entry.kind, offset)
                .unwrap();
            let next = destination
                .write_archive("restored", entry.kind, offset, &bytes)
                .unwrap();
            assert_eq!(
                destination
                    .write_archive("restored", entry.kind, offset, &bytes)
                    .unwrap(),
                next
            );
            offset = next;
        }
    }
    destination.finish_restore("restored").await.unwrap();
    destination.finish_restore("restored").await.unwrap();
    let restored = destination
        .environments
        .store
        .instance(&reserved.request.instance)
        .unwrap();
    let restored_handle = restored.handle.clone().unwrap();
    let mut target_b = target.clone();
    target_b.host_id = destination.environments.store.host().into();
    target_b.instance_id = restored.id.to_string();
    target_b.configuration_revision = restored.configuration.revision;
    target_b.identity.attempt_id = "attempt-example-b".into();
    target_b.identity.generation = 2;
    let mut running = restored.clone();
    running.target = Some(target_b.clone());
    destination
        .environments
        .store
        .save_instance(&running)
        .unwrap();
    destination
        .store
        .bind_execution("restored", &target_b)
        .unwrap();
    destination.store.started("restored").unwrap();
    destination
        .environments
        .docker
        .start(&restored_handle)
        .await
        .unwrap();
    assert_eq!(command(&destination,&restored_handle,&format!("cat /beale-workspace/workspace/edit.txt /beale-installed-example {}/state/plugins/example/tool.txt",restored_handle.runtime_directory)).await,"private editinstalled toolingprepared plugin");
    assert!(
        destination
            .environments
            .docker
            .stop(&restored_handle)
            .await
            .unwrap()
    );
    destination.reconcile(&reserved).await.unwrap();
    destination
        .environments
        .docker
        .start(&restored_handle)
        .await
        .unwrap();
    command(&destination, &restored_handle, "printf 'newest edit' > /beale-workspace/workspace/edit.txt; printf 'newest tooling' > /beale-installed-example").await;
    assert!(
        destination
            .environments
            .docker
            .stop(&restored_handle)
            .await
            .unwrap()
    );
    let checkpoint_b = destination.capture(&target_b, false).await.unwrap();
    destination
        .accept_checkpoint(&target_b, &archives::bundle_digest(&checkpoint_b).unwrap())
        .await
        .unwrap();
    let bundle_b = destination.capture(&target_b, true).await.unwrap();
    let mut back = spec("returned-to-a");
    back.restore = Some(bundle_b.clone());
    enqueue(&source, back.clone()).await;
    let returning = source.prepare_next().await.unwrap().unwrap();
    assert_ne!(returning.request.instance, target.instance_id);
    assert_eq!(
        enqueue(&source, back).await.request.instance,
        returning.request.instance
    );
    source.begin_restore("returned-to-a", &bundle_b).unwrap();
    for entry in &bundle_b.archives {
        let mut offset = 0;
        while offset < entry.bytes {
            let bytes = destination
                .read_archive(&archives::bundle_id(&bundle_b).unwrap(), entry.kind, offset)
                .unwrap();
            offset = source
                .write_archive("returned-to-a", entry.kind, offset, &bytes)
                .unwrap();
        }
    }
    source.finish_restore("returned-to-a").await.unwrap();
    let returned = source
        .environments
        .store
        .instance(&returning.request.instance)
        .unwrap();
    let returned_handle = returned.handle.unwrap();
    source
        .environments
        .docker
        .start(&returned_handle)
        .await
        .unwrap();
    assert_eq!(
        command(
            &source,
            &returned_handle,
            "cat /beale-workspace/workspace/edit.txt /beale-installed-example"
        )
        .await,
        "newest editnewest tooling"
    );
    assert!(
        source
            .environments
            .docker
            .stop(&returned_handle)
            .await
            .unwrap()
    );
    source.reconcile(&returning).await.unwrap();
    destination
        .cancel_export(&target_b, &target_b.identity.attempt_id)
        .unwrap();
    assert!(destination.capture(&target_b, true).await.is_err());
    let retried = destination
        .capture_export(&target_b, "retry-transfer-example")
        .await
        .unwrap();
    assert_eq!(retried, bundle_b);
    assert!(
        !destination
            .store
            .export_canceled(&target_b, "retry-transfer-example")
            .unwrap()
    );
    destination
        .cancel_export(&target_b, "retry-transfer-example")
        .unwrap();
    let env = source.store.environment(&target.instance_id).unwrap();
    source
        .store
        .retention(
            &env.instance,
            env.generation,
            RetentionAction::Release {
                reference: "restore-attempt-example".into(),
            },
            Allocator::now(),
        )
        .unwrap();
    for a in [&source, &destination] {
        for env in a.store.environments().unwrap() {
            a.store
                .retention(
                    &env.instance,
                    env.generation,
                    RetentionAction::ClaimDelete { now: true },
                    Allocator::now(),
                )
                .unwrap();
        }
        a.cleanup().await.unwrap();
        a.cleanup().await.unwrap();
        assert!(
            a.store
                .environments()
                .unwrap()
                .iter()
                .all(|e| e.deletion == DeletionStep::Deleted)
        );
    }
    // A deleted old copy is also a tombstone, not a prohibition on a new
    // explicitly restored generation.
    let mut after_delete = spec("return-after-deletion");
    after_delete.restore = Some(bundle_b);
    let fresh = enqueue(&source, after_delete).await;
    assert_ne!(fresh.request.instance, returning.request.instance);
    source.store.cancel_queued(&fresh.request.id).unwrap();
    // Removing private environments cannot remove the imported/shared base image.
    let out = destination
        .environments
        .docker
        .command()
        .args(["image", "inspect", bundle.image_digest.as_ref().unwrap()])
        .output()
        .await
        .unwrap();
    assert!(out.status.success());
    drop(destination);
    drop(source);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn changed_configuration_is_rejected_before_reserved_instance_preparation() {
    let root = std::env::temp_dir().join(format!("beale-config-example-{}", uuid::Uuid::new_v4()));
    let a = allocator(&root);
    let queued = enqueue(&a, spec("revision-example")).await;
    let mut config = a
        .environments
        .store
        .configuration("image-example")
        .unwrap()
        .configuration;
    config.revision += 1;
    config.image = "ubuntu:24.04".into();
    a.environments.store.save_configuration(config).unwrap();
    let error = a.prepare_next().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("reserved configuration revision changed")
    );
    let row = a.store.allocation(&queued.request.id).unwrap();
    assert_eq!(row.request.configuration_revision, 1);
    assert_eq!(row.phase, AllocationPhase::Retained);
    assert!(!row.prepared && row.execution.is_none());
    assert!(
        a.environments
            .store
            .instance_if_present(row.request.instance.parse().unwrap())
            .unwrap()
            .is_none()
    );
    assert!(
        a.store
            .environment(&row.request.instance)
            .unwrap()
            .references
            .is_empty()
    );
    http::handle(
        &a,
        HostAllocationRequest::Cancel {
            id: row.request.id.clone(),
        },
    )
    .await
    .unwrap();
    let retry = enqueue(&a, spec("new-revision-retry")).await;
    assert_ne!(retry.request.instance, row.request.instance);
    assert_eq!(retry.request.configuration_revision, 2);
    let admitted = a
        .store
        .admit_next(&a.capacity().await.unwrap(), Allocator::now())
        .unwrap()
        .unwrap();
    assert_eq!(admitted.request.id, retry.request.id);
    drop(a);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn interrupted_exports_are_deleted_by_owned_journal_without_manifest() {
    let root = std::env::temp_dir().join(format!(
        "beale-partial-archive-example-{}",
        uuid::Uuid::new_v4()
    ));
    let a = allocator(&root);
    let queued = enqueue(&a, spec("archive-example")).await;
    a.store
        .admit_next(&a.capacity().await.unwrap(), Allocator::now())
        .unwrap()
        .unwrap();
    a.store.confirm_stopped(&queued.request.id, true).unwrap();
    let id = "a".repeat(64);
    a.store.bind_archive(&id, &queued.request.instance).unwrap();
    let partial = root.join("allocation-archives").join(&id);
    std::fs::create_dir_all(&partial).unwrap();
    std::fs::write(
        partial.join("workspace.tar"),
        b"synthetic completed private component",
    )
    .unwrap();
    std::fs::write(
        partial.join("image.part"),
        b"synthetic interrupted component",
    )
    .unwrap();
    assert!(!partial.join("manifest.json").exists());
    let unrelated = root.join("allocation-archives").join("b".repeat(64));
    std::fs::create_dir_all(&unrelated).unwrap();
    std::fs::write(unrelated.join("workspace.tar"), b"another owner's archive").unwrap();
    let reopened =
        beale_storage::fleet_allocation::AllocationStore::open(HostDatabase::open(&root).unwrap())
            .unwrap();
    assert_eq!(
        reopened.archive_ids(&queued.request.instance).unwrap(),
        vec![id]
    );
    let e = reopened.environment(&queued.request.instance).unwrap();
    reopened
        .retention(
            &e.instance,
            e.generation,
            RetentionAction::ClaimDelete { now: true },
            Allocator::now(),
        )
        .unwrap();
    a.cleanup().await.unwrap();
    a.cleanup().await.unwrap();
    assert_eq!(
        reopened.environment(&e.instance).unwrap().deletion,
        DeletionStep::Deleted
    );
    assert!(!partial.exists());
    assert!(unrelated.join("workspace.tar").exists());
    drop(reopened);
    drop(a);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn client_inventory_reconstructs_allocations_separately_from_configurations() {
    let root =
        std::env::temp_dir().join(format!("beale-inventory-example-{}", uuid::Uuid::new_v4()));
    let a = allocator(&root);
    let queued = enqueue(&a, spec("allocation-example")).await;
    let reply = http::handle(&a, HostAllocationRequest::Inventory)
        .await
        .unwrap();
    let HostAllocationReply::Inventory {
        allocations,
        environments,
    } = reply
    else {
        panic!("expected inventory");
    };
    assert_eq!(allocations.len(), 1);
    assert_eq!(allocations[0].request.instance, queued.request.instance);
    assert_eq!(allocations[0].phase, queued.phase);
    assert!(environments.is_empty());
    drop(a);
    let reopened = allocator(&root);
    let HostAllocationReply::Inventory { allocations, .. } =
        http::handle(&reopened, HostAllocationRequest::Inventory)
            .await
            .unwrap()
    else {
        panic!("expected inventory");
    };
    assert_eq!(allocations.len(), 1);
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}
