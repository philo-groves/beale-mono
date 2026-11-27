use super::*;
use beale_contracts::{
    swarm_container::*, swarm_host::ExecutionTarget, swarm_worker::ExecutionIdentity,
};
use beale_storage::HostDatabase;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

fn root() -> PathBuf {
    let p = std::env::temp_dir().join(format!("beale-docker-example-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&p).unwrap();
    p
}
fn config() -> ImageConfiguration {
    ImageConfiguration {
        version: 1,
        id: "image-example".into(),
        revision: 1,
        image: "example/image:latest".into(),
        architecture: LinuxArchitecture::Amd64,
        options: ContainerOptions::default(),
    }
}
#[test]
fn configuration_validation_revision_and_host_readiness_are_durable() {
    let root = root();
    let store = DockerStore::open(HostDatabase::open(&root).unwrap()).unwrap();
    let mut config = config();
    assert_eq!(
        store.save_configuration(config.clone()).unwrap().state,
        Readiness::Unprepared
    );
    store
        .readiness(
            &config,
            Readiness::Unavailable,
            Some("Registry access requires this host's login"),
            None,
        )
        .unwrap();
    assert_eq!(
        store.save_configuration(config.clone()).unwrap().state,
        Readiness::Unavailable
    );
    config.image = "example/other:latest".into();
    assert!(store.save_configuration(config.clone()).is_err());
    config.revision = 2;
    store.save_configuration(config.clone()).unwrap();
    config.options.command = vec!["unexpected".into()];
    assert!(config.validate().is_err());
    config.options.startup = StartupMode::ImageDefault;
    assert!(config.validate().is_ok());
    config
        .options
        .environment
        .insert("BAD=KEY".into(), "value".into());
    assert!(config.validate().is_err());
    drop(store);
    let store = DockerStore::open(HostDatabase::open(&root).unwrap()).unwrap();
    assert_eq!(store.configurations().unwrap()[0].configuration.revision, 2);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn instance_ownership_requires_confirmed_stop_and_same_logical_agent() {
    let root = root();
    let store = DockerStore::open(HostDatabase::open(&root).unwrap()).unwrap();
    let id = uuid::Uuid::new_v4();
    let image = ResolvedImage {
        requested: "example/image".into(),
        digest: format!("sha256:{}", "a".repeat(64)),
        architecture: LinuxArchitecture::Amd64,
    };
    let target = ExecutionTarget {
        host_id: store.host().into(),
        leader_id: "leader-example".into(),
        configuration_id: config().id,
        configuration_revision: 1,
        instance_id: id.to_string(),
        identity: ExecutionIdentity {
            workspace_id: "workspace-example".into(),
            scan_id: "scan-example".into(),
            agent_id: "agent-example".into(),
            attempt_id: "attempt-example".into(),
            generation: 1,
        },
    };
    let mut row = Instance {
        id,
        configuration: config(),
        image: image.clone(),
        handle: Some(ContainerHandle {
            host: store.host().into(),
            instance: id,
            id: "b".repeat(64),
            volume: format!("beale-swarm-workspace-{id}"),
            runtime_directory: format!("/beale-runtime-{id}"),
            image,
        }),
        target: Some(target.clone()),
        stopped: false,
        prepared: true,
    };
    store.insert(&row).unwrap();
    assert!(store.insert(&row).is_err());
    let mut next = target.clone();
    next.identity.attempt_id = "next-example".into();
    next.identity.generation = 2;
    assert!(store.validate_target(&next).is_err());
    row.stopped = true;
    store.save_instance(&row).unwrap();
    assert!(store.validate_target(&next).is_ok());
    next.identity.agent_id = "other-example".into();
    assert!(store.validate_target(&next).is_err());
    next = target.clone();
    next.configuration_revision = 2;
    assert!(store.validate_target(&next).is_err());
    let environments = DockerEnvironments::new(
        Docker::new("unavailable-docker-example".into()),
        store.clone(),
        root.clone(),
        None,
    )
    .unwrap();
    let owner = environments.lock(id).unwrap();
    assert!(environments.lock(id).is_err());
    drop(owner);
    assert!(environments.lock(id).is_ok());
    drop(environments);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
fn elf() -> Vec<u8> {
    let mut b = vec![0; 64];
    b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    b[16] = 2;
    b[18] = 62;
    b[20] = 1;
    b[52] = 64;
    b[54] = 56;
    b[58] = 64;
    b
}
#[test]
fn runtime_bundle_rejects_corruption_architecture_and_changed_package() {
    let root = root();
    let binary = elf();
    for name in ["beale-worker", "bash"] {
        std::fs::write(root.join(name), &binary).unwrap();
    }
    std::fs::write(root.join("BASH-COPYRIGHT"), b"Synthetic test license").unwrap();
    let artifact = |b: &[u8]| RuntimeArtifact {
        sha256: format!("{:x}", Sha256::digest(b)),
        bytes: b.len() as u64,
    };
    let mut manifest = RuntimeManifest {
        version: 1,
        worker_protocol: 1,
        architecture: LinuxArchitecture::Amd64,
        worker: artifact(&binary),
        shell: artifact(&binary),
        shell_license: artifact(b"Synthetic test license"),
    };
    let save = |m: &RuntimeManifest| {
        std::fs::write(root.join("manifest.json"), serde_json::to_vec(m).unwrap()).unwrap()
    };
    save(&manifest);
    let bundle = RuntimeBundle::open(&root).unwrap();
    manifest.architecture = LinuxArchitecture::Arm64;
    save(&manifest);
    assert!(RuntimeBundle::open(&root).is_err());
    manifest.architecture = LinuxArchitecture::Amd64;
    save(&manifest);
    std::fs::write(root.join("bash"), b"changed").unwrap();
    assert!(RuntimeBundle::open(&root).is_err());
    assert!(
        bundle
            .write_archive(
                &mut std::fs::File::create(root.join("archive.tar")).unwrap(),
                uuid::Uuid::new_v4()
            )
            .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn configuration_http_requires_host_authority_and_strict_versioned_documents() {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let root = root();
    let db = HostDatabase::open(&root).unwrap();
    let app = router("host-example".into(), db.clone());
    let get = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(CONFIGURATIONS_PATH)
                .header("authorization", "Bearer worker-example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::UNAUTHORIZED);
    let post = |value: serde_json::Value| {
        Request::builder()
            .method("POST")
            .uri(CONFIGURATIONS_PATH)
            .header("authorization", "Bearer host-example")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&value).unwrap()))
            .unwrap()
    };
    assert_eq!(
        app.clone()
            .oneshot(post(serde_json::to_value(config()).unwrap()))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let mut invalid = serde_json::to_value(config()).unwrap();
    invalid["version"] = serde_json::json!(2);
    assert_eq!(
        app.clone().oneshot(post(invalid)).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let mut invalid = serde_json::to_value(config()).unwrap();
    invalid["host_path"] = serde_json::json!("unexpected");
    assert_eq!(
        app.clone().oneshot(post(invalid)).await.unwrap().status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        DockerStore::open(db.clone())
            .unwrap()
            .configurations()
            .unwrap()
            .len(),
        1
    );
    drop(app);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn missing_daemon_never_proves_container_termination() {
    let docker = Docker::new("unavailable-docker-example".into());
    let id = uuid::Uuid::new_v4();
    let handle = ContainerHandle {
        host: "host-example".into(),
        instance: id,
        id: "b".repeat(64),
        volume: format!("beale-swarm-workspace-{id}"),
        runtime_directory: format!("/beale-runtime-{id}"),
        image: ResolvedImage {
            requested: "example/image".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            architecture: LinuxArchitecture::Amd64,
        },
    };
    assert!(docker.stop(&handle).await.is_err());
    assert!(docker.delete(&handle).await.is_err());
    assert!(
        docker
            .resolve_image("--bad", LinuxArchitecture::Amd64)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn missing_runtime_is_actionable_and_does_not_allocate() {
    let root = root();
    let store = DockerStore::open(HostDatabase::open(&root).unwrap()).unwrap();
    store.save_configuration(config()).unwrap();
    let env = DockerEnvironments::new(
        Docker::new("unavailable-docker-example".into()),
        store.clone(),
        root.clone(),
        None,
    )
    .unwrap();
    let id = uuid::Uuid::new_v4();
    assert!(env.prepare(&config().id, id).await.is_err());
    let readiness = store.configuration(&config().id).unwrap();
    assert_eq!(readiness.state, Readiness::Unavailable);
    assert!(
        readiness
            .reason
            .unwrap()
            .contains("portable worker runtime")
    );
    assert!(store.instance(&id.to_string()).is_err());
    drop(env);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

fn retained_instance(store: &DockerStore, id: uuid::Uuid) -> Instance {
    let image = ResolvedImage {
        requested: config().image,
        digest: format!("sha256:{}", "a".repeat(64)),
        architecture: LinuxArchitecture::Amd64,
    };
    Instance {
        id,
        configuration: config(),
        image: image.clone(),
        handle: Some(ContainerHandle {
            host: store.host().into(),
            instance: id,
            id: "b".repeat(64),
            volume: format!("beale-swarm-workspace-{id}"),
            runtime_directory: format!("/beale-runtime-{id}"),
            image,
        }),
        target: None,
        stopped: false,
        prepared: false,
    }
}
fn recovery_environment(root: &std::path::Path) -> DockerEnvironments {
    DockerEnvironments::new(
        Docker::new("unavailable-docker-example".into()),
        DockerStore::open(HostDatabase::open(root).unwrap()).unwrap(),
        root.into(),
        None,
    )
    .unwrap()
}
async fn await_readiness(env: &DockerEnvironments, expected: Readiness) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while env.store.configuration(&config().id).unwrap().state != expected {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn preparation_recovery_reloads_candidates_after_acquiring_ownership() {
    let root = root();
    let env = recovery_environment(&root);
    env.store.save_configuration(config()).unwrap();
    let id = uuid::Uuid::new_v4();
    let owner = env.lock(id).unwrap();
    env.store.begin_preparation(&config(), id).unwrap();
    let mut row = retained_instance(&env.store, id);
    env.store.insert(&row).unwrap();
    // Recovery selected this row while preparation still owned it. Complete
    // preparation before the selected candidate acquires its instance lock.
    let candidates = env.store.unfinished("").unwrap();
    assert_eq!(candidates, [id]);
    row.prepared = true;
    row.stopped = true;
    row.target = Some(ExecutionTarget {
        host_id: env.store.host().into(),
        leader_id: "leader-example".into(),
        configuration_id: config().id,
        configuration_revision: 1,
        instance_id: id.to_string(),
        identity: ExecutionIdentity {
            workspace_id: "workspace-example".into(),
            scan_id: "scan-example".into(),
            agent_id: "agent-example".into(),
            attempt_id: "attempt-example".into(),
            generation: 1,
        },
    });
    env.store.save_instance(&row).unwrap();
    env.store
        .finish_preparation(
            id,
            &config(),
            Readiness::Ready,
            None,
            Some(&row.image.digest),
        )
        .unwrap();
    drop(owner);
    env.recover_preparation(candidates[0]).await.unwrap();
    let recovered = env.store.instance(&id.to_string()).unwrap();
    assert!(recovered.prepared && recovered.stopped);
    assert_eq!(recovered.target, row.target);
    assert_eq!(
        env.store.configuration(&config().id).unwrap().state,
        Readiness::Ready
    );
    env.store
        .validate_target(row.target.as_ref().unwrap())
        .unwrap();
    drop(env);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn preparation_journal_recovers_interrupted_pull_without_disturbing_live_owner() {
    let root = root();
    let env = recovery_environment(&root);
    env.store.save_configuration(config()).unwrap();
    let id = uuid::Uuid::new_v4();
    let owner = env.lock(id).unwrap();
    env.store.begin_preparation(&config(), id).unwrap();
    // This is the durable state while resolve_image is awaiting a pull.
    env.recover_preparations(&mut String::new()).await.unwrap();
    assert_eq!(
        env.store.configuration(&config().id).unwrap().state,
        Readiness::Preparing
    );
    assert!(env.store.preparation(id).unwrap().is_some());
    drop(owner);
    drop(env);
    let env = recovery_environment(&root);
    let maintenance = env.start_recovery();
    await_readiness(&env, Readiness::Unavailable).await;
    assert!(
        env.store
            .configuration(&config().id)
            .unwrap()
            .reason
            .unwrap()
            .contains("before instance creation")
    );
    assert!(env.store.instance_if_present(id).unwrap().is_none());
    assert!(env.store.preparation(id).unwrap().is_none());
    maintenance.abort();
    let _ = maintenance.await;
    drop(env);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn preparation_journal_retries_failed_final_readiness_write_after_restart() {
    let root = root();
    let env = recovery_environment(&root);
    env.store.save_configuration(config()).unwrap();
    let id = uuid::Uuid::new_v4();
    let owner = env.lock(id).unwrap();
    env.store.begin_preparation(&config(), id).unwrap();
    let mut row = retained_instance(&env.store, id);
    row.prepared = true;
    row.stopped = true;
    env.store.insert(&row).unwrap();
    let db = HostDatabase::open(&root).unwrap();
    db.with_connection(|db| { db.execute_batch("CREATE TRIGGER reject_readiness BEFORE UPDATE ON fleet_docker_configurations WHEN json_extract(NEW.document,'$.state')='ready' BEGIN SELECT RAISE(ABORT,'synthetic readiness storage failure'); END;")?; Ok(()) }).unwrap();
    assert!(
        env.store
            .finish_preparation(
                id,
                &config(),
                Readiness::Ready,
                None,
                Some(&row.image.digest)
            )
            .is_err()
    );
    assert!(env.store.preparation(id).unwrap().is_some());
    assert_eq!(
        env.store.configuration(&config().id).unwrap().state,
        Readiness::Preparing
    );
    db.with_connection(|db| {
        db.execute_batch("DROP TRIGGER reject_readiness")?;
        Ok(())
    })
    .unwrap();
    drop(db);
    drop(owner);
    drop(env);
    let env = recovery_environment(&root);
    let maintenance = env.start_recovery();
    await_readiness(&env, Readiness::Ready).await;
    assert!(env.store.preparation(id).unwrap().is_none());
    let recovered = env.store.instance(&id.to_string()).unwrap();
    assert!(recovered.prepared && recovered.stopped);
    assert_eq!(recovered.handle.unwrap().id, row.handle.unwrap().id);
    maintenance.abort();
    let _ = maintenance.await;
    drop(env);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_preparation_cannot_overwrite_newer_preparation_or_configuration_readiness() {
    let root = root();
    let env = recovery_environment(&root);
    env.store.save_configuration(config()).unwrap();
    let first = uuid::Uuid::new_v4();
    let second = uuid::Uuid::new_v4();
    env.store.begin_preparation(&config(), first).unwrap();
    env.store.begin_preparation(&config(), second).unwrap();
    env.store
        .finish_preparation(
            first,
            &config(),
            Readiness::Unavailable,
            Some("interrupted"),
            None,
        )
        .unwrap();
    assert_eq!(
        env.store.configuration(&config().id).unwrap().state,
        Readiness::Preparing
    );
    let mut next = config();
    next.revision = 2;
    env.store.save_configuration(next).unwrap();
    env.store
        .finish_preparation(second, &config(), Readiness::Ready, None, None)
        .unwrap();
    assert_eq!(
        env.store.configuration(&config().id).unwrap().state,
        Readiness::Unprepared
    );
    assert!(env.store.unfinished("").unwrap().is_empty());
    drop(env);
    std::fs::remove_dir_all(root).unwrap();
}

fn bound_preflight_instance(store: &DockerStore) -> Instance {
    let mut row = retained_instance(store, uuid::Uuid::new_v4());
    row.prepared = true;
    row.target = Some(ExecutionTarget {
        host_id: store.host().into(),
        leader_id: "leader-example".into(),
        configuration_id: config().id,
        configuration_revision: 1,
        instance_id: row.id.to_string(),
        identity: ExecutionIdentity {
            workspace_id: "workspace-example".into(),
            scan_id: "scan-example".into(),
            agent_id: "agent-example".into(),
            attempt_id: uuid::Uuid::new_v4().to_string(),
            generation: 1,
        },
    });
    store.insert(&row).unwrap();
    row
}

#[test]
fn preflight_success_clears_failure_and_is_durable_and_single_use() {
    let root = root();
    let store = DockerStore::open(HostDatabase::open(&root).unwrap()).unwrap();
    store.save_configuration(config()).unwrap();
    let row = bound_preflight_instance(&store);
    let target = row.target.as_ref().unwrap();
    let failed = store.begin_preflight(target).unwrap().unwrap();
    assert!(
        store
            .complete_preflight(&failed, Some("synthetic leader unavailable"))
            .unwrap()
    );
    assert_eq!(
        store.configuration(&config().id).unwrap().state,
        Readiness::Unavailable
    );
    let success = store.begin_preflight(target).unwrap().unwrap();
    assert!(store.complete_preflight(&success, None).unwrap());
    assert!(
        !store
            .complete_preflight(&failed, Some("late failure"))
            .unwrap()
    );
    assert!(
        !store
            .complete_preflight(&success, Some("duplicate failure"))
            .unwrap()
    );
    drop(store);
    let store = DockerStore::open(HostDatabase::open(&root).unwrap()).unwrap();
    let readiness = store.configuration(&config().id).unwrap();
    assert_eq!(readiness.state, Readiness::Ready);
    assert_eq!(readiness.reason, None);
    assert_eq!(readiness.image_digest, Some(row.image.digest));
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn preflight_results_cannot_overwrite_newer_attempt_preparation_or_configuration() {
    let root = root();
    let store = DockerStore::open(HostDatabase::open(&root).unwrap()).unwrap();
    store.save_configuration(config()).unwrap();
    let mut first = bound_preflight_instance(&store);
    let second = bound_preflight_instance(&store);
    let old = store
        .begin_preflight(first.target.as_ref().unwrap())
        .unwrap()
        .unwrap();
    let newer = store
        .begin_preflight(second.target.as_ref().unwrap())
        .unwrap()
        .unwrap();
    assert!(
        store
            .complete_preflight(&newer, Some("newer failure"))
            .unwrap()
    );
    assert!(!store.complete_preflight(&old, None).unwrap());
    assert_eq!(
        store.configuration(&config().id).unwrap().reason.as_deref(),
        Some("newer failure")
    );
    let old = store
        .begin_preflight(first.target.as_ref().unwrap())
        .unwrap()
        .unwrap();
    let target = first.target.as_mut().unwrap();
    target.identity.generation += 1;
    target.identity.attempt_id = "replacement-example".into();
    store.save_instance(&first).unwrap();
    assert!(!store.complete_preflight(&old, None).unwrap());
    let old = store
        .begin_preflight(first.target.as_ref().unwrap())
        .unwrap()
        .unwrap();
    let preparation = uuid::Uuid::new_v4();
    store.begin_preparation(&config(), preparation).unwrap();
    assert!(!store.complete_preflight(&old, None).unwrap());
    store
        .finish_preparation(
            preparation,
            &config(),
            Readiness::Ready,
            None,
            Some(&first.image.digest),
        )
        .unwrap();
    assert!(
        !store
            .complete_preflight(&old, Some("late failure"))
            .unwrap()
    );
    let old = store
        .begin_preflight(first.target.as_ref().unwrap())
        .unwrap()
        .unwrap();
    let mut next = config();
    next.revision = 2;
    store.save_configuration(next).unwrap();
    assert!(!store.complete_preflight(&old, None).unwrap());
    assert!(
        store
            .begin_preflight(first.target.as_ref().unwrap())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.configuration(&config().id).unwrap().state,
        Readiness::Unprepared
    );
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn execution_peer_registration_is_host_only_and_never_echoes_pairing_material() {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use beale_contracts::swarm_ui::{PEER_REGISTRATION_PATH, PeerRegistration};
    use tower::ServiceExt;
    let root = root();
    let db = HostDatabase::open(&root).unwrap();
    let app = router("host-example".into(), db.clone());
    let peer = PeerRegistration {
        host_id: "peer-example".into(),
        endpoint: "https://example-host.example-tailnet.ts.net".into(),
        control_token: "a".repeat(64),
        enabled: true,
        protocol_major: beale_contracts::PROTOCOL_MAJOR,
    };
    let request = |token: &str, bytes: Vec<u8>| {
        Request::builder()
            .method("POST")
            .uri(PEER_REGISTRATION_PATH)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(bytes))
            .unwrap()
    };
    let denied = app
        .clone()
        .oneshot(request(
            "worker-example",
            serde_json::to_vec(&peer).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let saved = app
        .clone()
        .oneshot(request("host-example", serde_json::to_vec(&peer).unwrap()))
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(
        &to_bytes(saved.into_body(), 1024).await.unwrap()[..],
        b"true"
    );
    let store =
        beale_storage::fleet_coordination::FleetCoordinationStore::open(db.clone()).unwrap();
    assert_eq!(
        store.peer("peer-example").unwrap().unwrap().control_token,
        peer.control_token
    );
    let mut invalid = peer;
    invalid.endpoint = "http://example.test".into();
    assert_eq!(
        app.oneshot(request(
            "host-example",
            serde_json::to_vec(&invalid).unwrap()
        ))
        .await
        .unwrap()
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        store.peer("peer-example").unwrap().unwrap().endpoint,
        "https://example-host.example-tailnet.ts.net/"
    );
    drop(store);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_preflight_success_write_remains_retryable() {
    let root = root();
    let db = HostDatabase::open(&root).unwrap();
    let store = DockerStore::open(db.clone()).unwrap();
    store.save_configuration(config()).unwrap();
    let row = bound_preflight_instance(&store);
    let observation = store
        .begin_preflight(row.target.as_ref().unwrap())
        .unwrap()
        .unwrap();
    db.with_connection(|db| { db.execute_batch("CREATE TRIGGER reject_preflight BEFORE UPDATE ON fleet_docker_configurations WHEN json_extract(NEW.document,'$.state')='ready' BEGIN SELECT RAISE(ABORT,'synthetic readiness storage failure'); END;")?; Ok(()) }).unwrap();
    assert!(store.complete_preflight(&observation, None).is_err());
    db.with_connection(|db| {
        db.execute_batch("DROP TRIGGER reject_preflight")?;
        Ok(())
    })
    .unwrap();
    assert!(store.complete_preflight(&observation, None).unwrap());
    assert_eq!(
        store.configuration(&config().id).unwrap().state,
        Readiness::Ready
    );
    drop(store);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}
