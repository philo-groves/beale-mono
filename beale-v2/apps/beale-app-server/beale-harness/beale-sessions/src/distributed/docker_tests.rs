//! Opt-in Docker acceptance. Uses only synthetic provider and workspace data.
//! BEALE_CONTAINER_RUNTIME selects a packaged static worker; Docker Desktop
//! tests also set BEALE_DOCKER_TEST_GATEWAY to its host-gateway IP. The test-only
//! /etc/hosts mapping lets the same loopback leader URL work on host and guest.
use super::*;
use crate::distributed::{docker::DockerBackend, peer_host::AssignedWorkerBackend};
use beale_contracts::{swarm_container::*, swarm_host::*, swarm_worker::WorkerShell};
use beale_fleet::{docker::*, worker_host::HostExecutionBackend};
use beale_storage::{fleet_executions::BackendObservation, swarm_files::ResearchFiles};
use std::sync::atomic::AtomicUsize;

fn environments(root: &std::path::Path) -> DockerEnvironments {
    let bundle = std::env::var_os("BEALE_CONTAINER_RUNTIME")
        .expect("set BEALE_CONTAINER_RUNTIME for Docker acceptance");
    DockerEnvironments::new(
        Docker::new(
            std::env::var_os("BEALE_DOCKER_CLI")
                .map(PathBuf::from)
                .unwrap_or_else(|| "docker".into()),
        ),
        DockerStore::open(HostDatabase::open(root).unwrap()).unwrap(),
        root.to_path_buf(),
        Some(bundle.into()),
    )
    .unwrap()
}
async fn command(docker: &Docker, handle: &ContainerHandle, script: &str) -> String {
    let out = docker
        .command()
        .args([
            "exec",
            "--",
            &handle.id,
            &format!("{}/bash", handle.runtime_directory),
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
        "container fixture command failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
async fn stopped(backend: &AssignedWorkerBackend, target: &ExecutionTarget) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        if backend.inspect(target).await.unwrap() == BackendObservation::Stopped {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "container did not stop"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
async fn publish_snapshot(f: &Fixture) -> String {
    use beale_contracts::swarm_files::*;
    let files = ResearchFiles::open(f.state.db.clone(), f.state.workspaces.root()).unwrap();
    let bytes = b"printf 'seed metadata verified\\n'\n";
    let artifact = Artifact {
        id: "seed-example".into(),
        operation: "seed-example".into(),
        workspace: f.workspace_id.clone(),
        scan: "scan-example".into(),
        producer: f.deputy.clone(),
        assignment: None,
        attempt: f.attempt.id.clone(),
        generation: f.attempt.generation,
        revision: 1,
        source_path: "seed.sh".into(),
        sha256: beale_storage::swarm_files::hash(bytes),
        size: bytes.len() as u64,
        executable: Some(true),
        previous: None,
    };
    files.begin_transfer(&artifact).unwrap();
    files.write_chunk(&artifact.id, 0, bytes).unwrap();
    files.seal(&artifact.id).unwrap();
    let manifest = Manifest {
        version: 1,
        entries: vec![
            Entry {
                path: "seed.sh".into(),
                kind: EntryKind::File {
                    sha256: artifact.sha256,
                    size: artifact.size,
                    executable: Some(true),
                    mode: Some(0o751),
                },
            },
            Entry {
                path: "seed-link".into(),
                kind: EntryKind::Symlink {
                    target: "seed.sh".into(),
                    directory: false,
                },
            },
            Entry {
                path: "empty".into(),
                kind: EntryKind::Directory { mode: Some(0o750) },
            },
        ],
        repositories: vec![],
    };
    files.import_snapshot("scan-example", &manifest).unwrap()
}
#[cfg(windows)]
fn capture_windows_worktree(f: &Fixture) -> String {
    use beale_storage::swarm_files::SnapshotInput;
    let source = f.root.join("git-source");
    std::fs::create_dir(&source).unwrap();
    let git = |root: &std::path::Path, args: &[&str]| {
        let result = std::process::Command::new("git")
            .current_dir(root)
            .args([
                "-c",
                "user.name=Example",
                "-c",
                "user.email=example@example.test",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap()
    };
    git(&source, &["init", "--quiet"]);
    git(&source, &["config", "core.symlinks", "false"]);
    std::fs::write(
        source.join("seed.sh"),
        "printf 'Windows snapshot executed'\n",
    )
    .unwrap();
    std::fs::write(source.join("seed-link"), "seed.sh").unwrap();
    git(&source, &["add", "seed.sh", "seed-link"]);
    git(&source, &["update-index", "--chmod=+x", "seed.sh"]);
    let object = git(&source, &["hash-object", "-w", "seed-link"]);
    git(
        &source,
        &[
            "update-index",
            "--cacheinfo",
            &format!("120000,{},seed-link", object.trim()),
        ],
    );
    git(
        &source,
        &["commit", "--quiet", "-m", "Synthetic portable worktree"],
    );
    let worktree = f.root.join("linked-worktree");
    git(
        &source,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "example-branch",
            worktree.to_str().unwrap(),
        ],
    );
    assert!(worktree.join(".git").is_file());
    std::fs::create_dir(worktree.join("empty")).unwrap();
    let files = ResearchFiles::open(f.state.db.clone(), f.state.workspaces.root()).unwrap();
    let snapshot = files
        .capture(
            "scan-example",
            &[SnapshotInput {
                source: worktree.clone(),
                destination: String::new(),
            }],
        )
        .unwrap();
    // The remote worker cannot accidentally consult the original repository.
    std::fs::rename(&source, f.root.join("source-moved")).unwrap();
    std::fs::rename(&worktree, f.root.join("worktree-moved")).unwrap();
    snapshot
}

async fn provider(
    f: &Fixture,
    pause: bool,
    custom_path: bool,
    windows_snapshot: bool,
) -> (tokio::task::JoinHandle<()>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let app=axum::Router::new().route("/v1/chat/completions",axum::routing::post(move |axum::Json(body):axum::Json<Value>| {let counter=counter.clone();async move {
        let call=counter.fetch_add(1,Ordering::SeqCst);
        if pause {tokio::time::sleep(Duration::from_secs(60)).await;}
        let delta=if call.is_multiple_of(2) {
            let setup = if windows_snapshot {
                r#"test -x seed.sh && test -L seed-link && test -d empty && test -d .git && test ! -f .git/commondir && test ! -f .git/gitdir && test "$(git symbolic-ref HEAD)" = refs/heads/example-branch && git log --oneline | grep -q 'Synthetic portable worktree' && bash seed-link"#
            } else {
                "test -x seed.sh && test -L seed-link && test -d empty && test $(stat -c %a seed.sh) = 751 && test $(stat -c %a empty) = 750"
            };
            let command = if call == 0 {
                format!("{setup} && printf 'retained workspace' > retained.txt && printf 'installed outside workspace' > /beale-installed-example && for ((i=0;i<20000;i++)); do printf 'Synthetic artifact bytes\\n'; done")
            } else {
                "test \"$(<retained.txt)\" = 'retained workspace' && test \"$(</beale-installed-example)\" = 'installed outside workspace' && printf 'retained state verified'".into()
            };
            let command = if custom_path {format!("test \"$(example-path-tool)\" = 'custom path works' && test \"$(</entrypoint-ready)\" = 'original entrypoint' && {command}")} else {command};
            json!({"tool_calls":[{"index":0,"id":format!("command-example-{call}"),"type":"function","function":{"name":"bash","arguments":json!({"command":command,"timeout_seconds":10}).to_string()}}]})
        } else { let message=body["messages"].as_array().unwrap().iter().rev().find(|m|m["role"]=="tool").unwrap(); let text=message["content"].as_str().unwrap(); assert!(text.contains(if call==1 {"Synthetic artifact bytes"} else {"retained state verified"}),"Unexpected synthetic command outcome: {text}"); json!({"content":"Synthetic Docker research complete"})};
        let chunk=json!({"choices":[{"delta":delta}]}); ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"))
    }}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    f.state
        .credentials
        .save_custom_provider(beale_providers::CustomProvider {
            id: "custom-example".into(),
            name: "Example".into(),
            base_url: format!("http://{}/v1", listener.local_addr().unwrap()),
            models: vec![beale_providers::ModelEntry {
                id: "example".into(),
                name: "Example".into(),
            }],
            lead_model: "example".into(),
            small_model: "example".into(),
            reasoning: false,
        })
        .unwrap();
    (
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }),
        calls,
    )
}
async fn round_trip(
    image: &str,
    startup: StartupMode,
    stop_case: Option<StopReason>,
    custom_path: Option<bool>,
) {
    round_trip_with_retry(image, startup, stop_case, custom_path, None).await;
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum PreflightRetry {
    LeaderRejection,
    EarlyFailures,
    WindowsSnapshot,
}
async fn round_trip_with_retry(
    image: &str,
    startup: StartupMode,
    stop_case: Option<StopReason>,
    custom_path: Option<bool>,
    retry_preflight: Option<PreflightRetry>,
) {
    let f = Fixture::with_shell(true, WorkerShell::Bash, true).await;
    let root = f.root.join("docker-host");
    let env = environments(&root);
    let id = Uuid::new_v4();
    let gateway = std::env::var("BEALE_DOCKER_TEST_GATEWAY")
        .expect("set BEALE_DOCKER_TEST_GATEWAY to the synthetic listener gateway");
    assert!(gateway.parse::<std::net::IpAddr>().is_ok());
    // Docker regenerates /etc/hosts on every start. This fixture-only startup
    // wrapper reestablishes the synthetic localhost mapping before PID 1.
    let application = if startup == StartupMode::ImageDefault {
        "printf 'application active' > /application-ready; exec tail -f /dev/null".to_owned()
    } else {
        format!("exec /beale-runtime-{id}/beale-worker container-idle")
    };
    let mut options = ContainerOptions {
        startup: StartupMode::ImageDefault,
        command: vec![
            "/bin/sh".into(),
            "-c".into(),
            format!(
                "printf '{gateway} localhost\\n' > /etc/hosts; if test -f /fail-start-example; then rm /fail-start-example; exit 1; fi; {application}"
            ),
        ],
        ..Default::default()
    };
    let custom_image = if custom_path.is_some() {
        Some(build_path_image(&env, image).await)
    } else {
        None
    };
    if custom_path == Some(true) {
        options
            .environment
            .insert("PATH".into(), "/opt/override/bin:/usr/bin:/bin".into());
    }
    env.store
        .save_configuration(ImageConfiguration {
            version: 1,
            id: "configuration-example".into(),
            revision: 1,
            image: custom_image
                .as_ref()
                .map_or_else(|| image.into(), |image| image.requested.clone()),
            architecture: LinuxArchitecture::Amd64,
            options,
        })
        .unwrap();
    // The locally built regression image has no registry tag to pull. Exercise
    // the same pinned-image preparation boundary used after image resolution.
    let row = if let Some(image) = &custom_image {
        let _owner = env.lock(id).unwrap();
        let config = env
            .store
            .configuration("configuration-example")
            .unwrap()
            .configuration;
        let mut row = Instance {
            id,
            configuration: config.clone(),
            image: image.clone(),
            handle: None,
            target: None,
            stopped: false,
            prepared: false,
        };
        env.store.insert(&row).unwrap();
        let bundle = RuntimeBundle::open(&PathBuf::from(
            std::env::var_os("BEALE_CONTAINER_RUNTIME").unwrap(),
        ))
        .unwrap();
        let archive = root.join("runtime-example.tar");
        let handle = env
            .docker
            .prepare_new(
                env.store.host(),
                id,
                image.clone(),
                &bundle,
                &archive,
                &config.options,
            )
            .await
            .unwrap();
        assert!(env.docker.stop(&handle).await.unwrap());
        row.handle = Some(handle);
        row.prepared = true;
        row.stopped = true;
        env.store.save_instance(&row).unwrap();
        env.store
            .readiness(&config, Readiness::Ready, None, Some(&image.digest))
            .unwrap();
        std::fs::remove_file(archive).unwrap();
        row
    } else {
        env.prepare("configuration-example", id).await.unwrap()
    };
    let handle = row.handle.unwrap();
    env.docker.start(&handle).await.unwrap();
    if retry_preflight == Some(PreflightRetry::WindowsSnapshot) {
        command(&env.docker, &handle, "apk add --no-cache git").await;
    }
    if startup == StartupMode::ImageDefault {
        assert_eq!(
            command(
                &env.docker,
                &handle,
                "printf '%s' \"$(</application-ready)\""
            )
            .await,
            "application active"
        );
    }
    assert!(env.docker.stop(&handle).await.unwrap());
    // A second listener exposes only this isolated synthetic worker gateway.
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let app = super::super::gateway::router().with_state(f.state.clone());
    let gateway_task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    env.docker.start(&handle).await.unwrap();
    let port = reqwest::Url::parse(&origin).unwrap().port().unwrap();
    let response=command(&env.docker,&handle,&format!("exec 3<>/dev/tcp/localhost/{port}; printf 'GET / HTTP/1.0\\r\\n\\r\\n' >&3; IFS= read -r line <&3; printf '%s' \"$line\"")).await;
    assert!(
        response.contains("404"),
        "synthetic gateway is unreachable from Docker: {response}"
    );
    assert!(env.docker.stop(&handle).await.unwrap());
    let windows_snapshot = retry_preflight == Some(PreflightRetry::WindowsSnapshot);
    #[cfg(windows)]
    let snapshot = if windows_snapshot {
        capture_windows_worktree(&f)
    } else {
        publish_snapshot(&f).await
    };
    #[cfg(not(windows))]
    let snapshot = publish_snapshot(&f).await;
    let (provider, calls) = provider(
        &f,
        stop_case.is_some(),
        custom_path.is_some(),
        windows_snapshot,
    )
    .await;
    let mut target = ExecutionTarget {
        host_id: env.store.host().into(),
        leader_id: "leader-example".into(),
        configuration_id: "configuration-example".into(),
        configuration_revision: 1,
        instance_id: handle.instance.to_string(),
        identity: f.store.worker_bootstrap(&f.attempt.id).unwrap().identity,
    };
    let mut launch = HostWorkerLaunch {
        leader_origin: origin,
        worker_bearer: f.credential.bearer().into(),
        snapshot: Some(snapshot),
    };
    if retry_preflight == Some(PreflightRetry::EarlyFailures) {
        use crate::distributed::peer_host::PreparedEnvironments;
        let adapter = DockerBackend(env.clone());
        // Cover all three early failure boundaries on the same retained
        // instance. The normal supervisor performs the subsequent valid turns.
        for phase in ["inspection", "startup", "runtime probe"] {
            // Each failure must replace a prior successful readiness result.
            env.store
                .readiness(
                    &row.configuration,
                    Readiness::Ready,
                    None,
                    Some(&row.image.digest),
                )
                .unwrap();
            let mut failing = env.clone();
            match phase {
                "inspection" => failing.docker = Docker::new("unavailable-docker-example".into()),
                "startup" => {
                    env.docker.start(&handle).await.unwrap();
                    command(&env.docker, &handle, "printf fail > /fail-start-example").await;
                    assert!(env.docker.stop(&handle).await.unwrap());
                }
                "runtime probe" => {
                    env.docker.start(&handle).await.unwrap();
                    command(
                        &env.docker,
                        &handle,
                        &format!("/bin/chmod 644 {}/bash", handle.runtime_directory),
                    )
                    .await;
                    assert!(env.docker.stop(&handle).await.unwrap());
                }
                _ => unreachable!(),
            }
            let error = match DockerBackend(failing).spawn(&target, &launch).await {
                Ok(_) => panic!("{phase} should reject research startup"),
                Err(error) => error,
            };
            let expected = match phase {
                "inspection" => "Docker CLI could not start",
                "startup" => "container did not remain running",
                "runtime probe" => {
                    "Docker operation failed; check daemon availability, registry access, and the selected image"
                }
                _ => unreachable!(),
            };
            assert_eq!(error.to_string(), expected, "failed at the wrong boundary");
            // Reopen the store to verify the failure is durable, not just a
            // return value that the supervisor could discard.
            let reopened = environments(&root);
            let readiness = reopened
                .store
                .configuration("configuration-example")
                .unwrap();
            assert_eq!(readiness.state, Readiness::Unavailable, "{phase}");
            assert_eq!(
                readiness.reason.as_deref(),
                Some(error.to_string().as_str()),
                "{phase}"
            );
            assert_eq!(
                readiness.image_digest.as_deref(),
                Some(row.image.digest.as_str())
            );
            assert_eq!(
                calls.load(Ordering::SeqCst),
                0,
                "{phase} cannot start research"
            );
            assert!(
                !reopened
                    .store
                    .instance(&target.instance_id)
                    .unwrap()
                    .stopped
            );
            let mut replacement = target.clone();
            replacement.identity.generation += 1;
            replacement.identity.attempt_id = "replacement-example".into();
            assert!(reopened.store.validate_target(&replacement).is_err());
            if phase == "runtime probe" {
                // Repair through the image's own tool, since the injected
                // shell deliberately cannot execute during this failure.
                let status = env
                    .docker
                    .command()
                    .args([
                        "exec",
                        "--",
                        &handle.id,
                        "/bin/chmod",
                        "755",
                        &format!("{}/bash", handle.runtime_directory),
                    ])
                    .status()
                    .await
                    .unwrap();
                assert!(status.success());
            }
            assert_eq!(
                adapter.recover(&target).await.unwrap(),
                BackendObservation::Stopped
            );
            assert_eq!(
                env.store
                    .configuration("configuration-example")
                    .unwrap()
                    .state,
                Readiness::Unavailable
            );
        }
    }
    if retry_preflight == Some(PreflightRetry::LeaderRejection) {
        use crate::distributed::peer_host::PreparedEnvironments;
        let adapter = DockerBackend(env.clone());
        let mut rejected = launch.clone();
        rejected.worker_bearer = "d".repeat(64);
        assert!(adapter.spawn(&target, &rejected).await.is_err());
        let readiness = env.store.configuration("configuration-example").unwrap();
        assert_eq!(readiness.state, Readiness::Unavailable);
        assert!(readiness.reason.is_some());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "failed preflight cannot start research"
        );
        assert_eq!(
            adapter.recover(&target).await.unwrap(),
            BackendObservation::Stopped
        );
    }
    if let Some(reason) = stop_case {
        use crate::distributed::peer_host::PreparedEnvironments;
        let adapter = DockerBackend(env.clone());
        let mut prepared = adapter.spawn(&target, &launch).await.unwrap();
        let fault_db = HostDatabase::open(&root).unwrap();
        if reason == StopReason::Canceled {
            fault_db.with_connection(|db|{db.execute_batch("CREATE TRIGGER reject_container_stop BEFORE UPDATE ON fleet_docker_instances WHEN json_extract(NEW.document,'$.stopped')=1 BEGIN SELECT RAISE(ABORT,'synthetic stop persistence full'); END;")?;Ok(())}).unwrap();
        }
        // A detached process outside the worker group must also be terminated.
        command(&env.docker, &handle, "sleep 300 >/dev/null 2>&1 & disown").await;
        let worker = WorkerLaunch {
            version: 1,
            identity: target.identity.clone(),
            leader: launch.leader_origin.clone(),
            bearer: launch.worker_bearer.clone(),
            workspace: prepared.workspace,
            protected_root: prepared.protected_root,
            recovery: prepared.recovery,
        };
        let identity = target.identity.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let supervisor_origin = format!("http://{}", listener.local_addr().unwrap());
        let first_lease = Arc::new(AtomicBool::new(true));
        let app = axum::Router::new().route(
            beale_contracts::swarm_worker::WORKER_PATH,
            axum::routing::post(move |axum::Json(request): axum::Json<WorkerRequest>| {
                let identity = identity.clone();
                let first = first_lease.clone();
                async move {
                    if !first.swap(false, Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    axum::Json(WorkerReply::Lease {
                        grant: beale_contracts::swarm_worker::LeaseGrant {
                            identity,
                            request_id: request.operation_id,
                            valid_for_ms: 200,
                        },
                    })
                }
            }),
        );
        let lease_server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (_cancel, canceled) = tokio::sync::watch::channel(reason == StopReason::Canceled);
        let report = supervise(
            prepared.process.as_mut(),
            target.identity.clone(),
            &WorkerTransport::new(&supervisor_origin, &launch.worker_bearer).unwrap(),
            &serde_json::to_vec(&worker).unwrap(),
            canceled,
        )
        .await
        .unwrap();
        assert_eq!(report.reason, reason);
        if reason == StopReason::Canceled {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            loop {
                if prepared
                    .process
                    .stop(Duration::from_millis(100))
                    .await
                    .is_err()
                {
                    break;
                }
                assert!(tokio::time::Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(!env.store.instance(&target.instance_id).unwrap().stopped);
            fault_db
                .with_connection(|db| {
                    db.execute_batch("DROP TRIGGER reject_container_stop")?;
                    Ok(())
                })
                .unwrap();
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while !prepared
            .process
            .stop(Duration::from_millis(20))
            .await
            .unwrap()
        {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            env.docker.inspect(&handle).await.unwrap(),
            ContainerStatus::Stopped
        );
        assert!(env.store.instance(&target.instance_id).unwrap().stopped);
        drop(fault_db);
        drop(prepared.process);
        lease_server.abort();
        // Simulate loss of the daemon connection while a retained instance is
        // live. Neither a missing CLI nor absence in a new backend proves stop.
        let mut row = env.store.instance(&target.instance_id).unwrap();
        row.stopped = false;
        env.store.save_instance(&row).unwrap();
        env.docker.start(&handle).await.unwrap();
        let mut broken = env.clone();
        broken.docker = Docker::new("unavailable-docker-example".into());
        assert_eq!(
            DockerBackend(broken).recover(&target).await.unwrap(),
            BackendObservation::TerminationUnconfirmed
        );
        let mut replacement = target.clone();
        replacement.identity.generation += 1;
        replacement.identity.attempt_id = "replacement-example".into();
        assert!(env.store.validate_target(&replacement).is_err());
        assert_eq!(
            adapter.recover(&target).await.unwrap(),
            BackendObservation::Stopped
        );
        // A later attempt canceled before spawn was polled must reconcile the
        // retained instance, rather than remain stuck on its previous target.
        assert_eq!(
            adapter.recover(&replacement).await.unwrap(),
            BackendObservation::Stopped
        );
        assert_eq!(
            env.store.instance(&target.instance_id).unwrap().target,
            Some(replacement)
        );
        // The production preparation-maintenance loop also stops interrupted
        // setup after a host restart without creating a replacement container.
        let mut row = env.store.instance(&target.instance_id).unwrap();
        row.stopped = false;
        row.prepared = false;
        row.target = None;
        env.store.save_instance(&row).unwrap();
        env.docker.start(&handle).await.unwrap();
        let recovery = env.start_recovery();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while !env.store.instance(&target.instance_id).unwrap().stopped {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        recovery.abort();
        assert!(!env.store.instance(&target.instance_id).unwrap().prepared);
        env.docker.delete(&handle).await.unwrap();
        provider.abort();
        gateway_task.abort();
        drop(adapter);
        drop(env);
        f.close().await;
        return;
    }
    for turn in 0..2 {
        // Reopen durable destination state between runs; never recreate Docker.
        let env = environments(&root);
        let backend =
            AssignedWorkerBackend::new(root.clone(), Arc::new(DockerBackend(env.clone())));
        backend.launch(&target, &launch).await.unwrap();
        stopped(&backend, &target).await;
        let readiness = env.store.configuration("configuration-example").unwrap();
        assert_eq!(readiness.state, Readiness::Ready);
        assert_eq!(readiness.reason, None);
        assert_eq!(
            readiness.image_digest.as_deref(),
            Some(handle.image.digest.as_str())
        );
        assert!(
            f.store
                .worker_progress(&target.identity.attempt_id)
                .unwrap()
                .answer
                .is_some(),
            "Docker worker failed before completing research: {:?}",
            env.store
                .configuration("configuration-example")
                .unwrap()
                .reason
        );
        assert_eq!(
            env.docker.inspect(&handle).await.unwrap(),
            ContainerStatus::Stopped
        );
        assert_eq!(
            env.store
                .instance(&target.instance_id)
                .unwrap()
                .handle
                .unwrap()
                .id,
            handle.id
        );
        f.store
            .reconcile_worker(
                &target.identity.attempt_id,
                true,
                WorkerDisposition::Completed,
                unix_ms(),
            )
            .unwrap();
        if turn == 0 {
            f.store
                .local_message(
                    "scan-example",
                    "related-example",
                    &f.deputy,
                    "Verify retained state",
                )
                .unwrap();
            let next = f
                .store
                .local_claim("scan-example", &f.deputy, unix_ms(), 64)
                .unwrap()
                .unwrap();
            let (_, credential) = prepare_worker(
                &f.state.db,
                &next,
                WorkerShell::Bash,
                &registration(),
                unix_ms(),
            )
            .unwrap();
            target.identity = f.store.worker_bootstrap(&next.attempt_id).unwrap().identity;
            launch.worker_bearer = credential.bearer().into();
            launch.snapshot = None;
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    let files = ResearchFiles::open(f.state.db.clone(), f.state.workspaces.root()).unwrap();
    let artifact_count = f
        .state
        .db
        .with_connection(|db| {
            Ok(db.query_row(
                "SELECT COUNT(*) FROM swarm_file_transfers WHERE accepted=1",
                [],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    assert!(
        artifact_count >= if windows_snapshot { 3 } else { 4 },
        "seed, readiness, and research output artifacts should be accepted"
    );
    drop(files);
    if startup == StartupMode::WorkerIdle {
        // Remove every conventional executable search directory in this
        // disposable fixture. Injected static binaries still pass readiness.
        env.docker.start(&handle).await.unwrap();
        command(
            &env.docker,
            &handle,
            "/bin/mv /usr/bin /usr/bin-disabled; /bin/mv /bin /bin-disabled",
        )
        .await;
        env.docker.probe(&handle).await.unwrap();
        assert!(env.docker.stop(&handle).await.unwrap());
    }
    let mut foreign = handle.clone();
    foreign.host = "foreign-example".into();
    assert!(env.docker.delete(&foreign).await.is_err());
    assert!(env.docker.inspect(&handle).await.is_ok());
    env.docker.delete(&handle).await.unwrap();
    assert!(env.docker.start(&handle).await.is_err());
    if let Some(image) = custom_image {
        let removed = env
            .docker
            .command()
            .args(["image", "rm", "--", &image.requested])
            .output()
            .await
            .unwrap();
        assert!(removed.status.success());
    }
    provider.abort();
    gateway_task.abort();
    drop(env);
    f.close().await;
}
#[tokio::test]
#[ignore = "requires Docker and a packaged static Linux worker"]
async fn docker_alpine_and_ubuntu_research_artifacts_metadata_and_retained_reuse() {
    tokio::join!(
        round_trip("alpine:3.22", StartupMode::WorkerIdle, None, None),
        round_trip("ubuntu:24.04", StartupMode::ImageDefault, None, None)
    );
}

#[tokio::test]
#[ignore = "requires Docker and a packaged static Linux worker"]
async fn docker_cancellation_lease_expiry_daemon_loss_and_background_recovery() {
    tokio::join!(
        round_trip(
            "alpine:3.22",
            StartupMode::WorkerIdle,
            Some(StopReason::Canceled),
            None
        ),
        round_trip(
            "ubuntu:24.04",
            StartupMode::ImageDefault,
            Some(StopReason::LeaseExpired),
            None
        )
    );
}

async fn build_path_image(env: &DockerEnvironments, base: &str) -> ResolvedImage {
    let context = env.root().join("path-image-example");
    std::fs::create_dir(&context).unwrap();
    let tag = format!("beale-path-example:{}", Uuid::new_v4());
    std::fs::write(context.join("Dockerfile"), format!(r#"FROM {base}
RUN mkdir -p /opt/example/bin /opt/override/bin
COPY tool.sh /opt/example/bin/example-path-tool
COPY tool.sh /opt/override/bin/example-path-tool
COPY entrypoint.sh /example-entrypoint
RUN chmod 755 /opt/example/bin/example-path-tool /opt/override/bin/example-path-tool /example-entrypoint
ENV PATH="/opt/example/bin:/usr/bin:/bin"
ENTRYPOINT ["/example-entrypoint"]
CMD ["tail", "-f", "/dev/null"]
"#)).unwrap();
    std::fs::write(
        context.join("tool.sh"),
        "#!/bin/sh\nprintf 'custom path works'\n",
    )
    .unwrap();
    std::fs::write(
        context.join("entrypoint.sh"),
        "#!/bin/sh\nprintf 'original entrypoint' > /entrypoint-ready\nexec \"$@\"\n",
    )
    .unwrap();
    let built = env
        .docker
        .command()
        .args(["build", "--tag", &tag])
        .arg(&context)
        .output()
        .await
        .unwrap();
    assert!(
        built.status.success(),
        "synthetic image build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let inspected = env
        .docker
        .command()
        .args(["image", "inspect", "--", &tag])
        .output()
        .await
        .unwrap();
    assert!(inspected.status.success());
    let value: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    ResolvedImage {
        requested: tag,
        digest: value[0]["Id"].as_str().unwrap().into(),
        architecture: LinuxArchitecture::Amd64,
    }
}

#[tokio::test]
#[ignore = "requires Docker and a packaged static Linux worker"]
async fn docker_image_entrypoint_and_custom_path_survive_research_and_reuse() {
    tokio::join!(
        round_trip("alpine:3.22", StartupMode::ImageDefault, None, Some(false)),
        round_trip("alpine:3.22", StartupMode::ImageDefault, None, Some(true))
    );
}

#[tokio::test]
#[ignore = "requires Docker and a packaged static Linux worker"]
async fn docker_preflight_retry_restores_readiness_after_leader_rejection() {
    round_trip_with_retry(
        "alpine:3.22",
        StartupMode::WorkerIdle,
        None,
        None,
        Some(PreflightRetry::LeaderRejection),
    )
    .await;
}

#[tokio::test]
#[ignore = "requires Docker and a packaged static Linux worker"]
async fn docker_early_preflight_failures_persist_readiness_and_retry() {
    round_trip_with_retry(
        "alpine:3.22",
        StartupMode::WorkerIdle,
        None,
        None,
        Some(PreflightRetry::EarlyFailures),
    )
    .await;
}

#[tokio::test]
#[ignore = "requires Docker and a packaged static Linux worker"]
async fn docker_default_idle_reaps_orphans_and_preserves_live_children() {
    let root = std::env::temp_dir().join(format!("beale-reaping-example-{}", Uuid::new_v4()));
    let env = environments(&root);
    let config = ImageConfiguration {
        version: 1,
        id: "reaping-example".into(),
        revision: 1,
        image: "alpine:3.22".into(),
        architecture: LinuxArchitecture::Amd64,
        options: ContainerOptions::default(),
    };
    env.store.save_configuration(config.clone()).unwrap();
    let row = env.prepare(&config.id, Uuid::new_v4()).await.unwrap();
    let handle = row.handle.unwrap();
    // Repeat after same-container restart to cover init signal registration.
    for _ in 0..2 {
        env.docker.start(&handle).await.unwrap();
        let pids = command(&env.docker, &handle, "for ((n=0;n<32;n++)); do /bin/sleep 0.2 </dev/null >/dev/null 2>&1 & printf '%s ' \"$!\"; done; /bin/sleep 60 </dev/null >/dev/null 2>&1 & printf '%s' \"$!\"").await;
        let mut pids: Vec<u32> = pids
            .split_whitespace()
            .map(|p| p.parse().unwrap())
            .collect();
        assert_eq!(pids.len(), 33);
        let live = pids.pop().unwrap();
        let exited = pids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        let script = format!(
            "for pid in {exited}; do if test -e /proc/$pid; then printf pending; exit 0; fi; done; kill -0 {live} && printf reaped"
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if command(&env.docker, &handle, &script).await == "reaped" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("init must reap exited orphan children while preserving live ones");
        assert_eq!(
            env.docker.inspect(&handle).await.unwrap(),
            ContainerStatus::Running
        );
        assert!(env.docker.stop(&handle).await.unwrap());
        assert_eq!(
            env.docker.inspect(&handle).await.unwrap(),
            ContainerStatus::Stopped
        );
    }
    env.docker.delete(&handle).await.unwrap();
    drop(env);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
#[ignore = "requires Docker and a packaged static Linux worker"]
async fn docker_default_idle_minimal_image_and_actionable_incompatibility() {
    let root =
        std::env::temp_dir().join(format!("beale-docker-preflight-example-{}", Uuid::new_v4()));
    let env = environments(&root);
    let config = ImageConfiguration {
        version: 1,
        id: "configuration-example".into(),
        revision: 1,
        image: "alpine:3.22".into(),
        architecture: LinuxArchitecture::Amd64,
        options: ContainerOptions::default(),
    };
    env.store.save_configuration(config.clone()).unwrap();
    let row = env.prepare(&config.id, Uuid::new_v4()).await.unwrap();
    let handle = row.handle.unwrap();
    env.docker.start(&handle).await.unwrap();
    command(&env.docker,&handle,"printf 'retained installation' > /installed-example; /bin/mv /usr/bin /usr/bin-disabled; /bin/mv /bin /bin-disabled").await;
    assert!(env.docker.stop(&handle).await.unwrap());
    env.docker.start(&handle).await.unwrap();
    env.docker.probe(&handle).await.unwrap();
    assert_eq!(
        command(
            &env.docker,
            &handle,
            "printf '%s' \"$(</installed-example)\""
        )
        .await,
        "retained installation"
    );
    assert!(env.docker.stop(&handle).await.unwrap());
    env.docker.delete(&handle).await.unwrap();

    let mut unsupported = config.clone();
    unsupported.id = "architecture-example".into();
    unsupported.architecture = LinuxArchitecture::Arm64;
    env.store.save_configuration(unsupported.clone()).unwrap();
    let id = Uuid::new_v4();
    assert!(env.prepare(&unsupported.id, id).await.is_err());
    assert!(
        env.store
            .configuration(&unsupported.id)
            .unwrap()
            .reason
            .unwrap()
            .contains("architecture")
    );
    assert!(env.store.instance(&id.to_string()).is_err());

    let mut denied = config.clone();
    denied.id = "unwritable-example".into();
    denied.options.user = Some("65534".into());
    env.store.save_configuration(denied.clone()).unwrap();
    let id = Uuid::new_v4();
    assert!(env.prepare(&denied.id, id).await.is_err());
    assert_eq!(
        env.store.configuration(&denied.id).unwrap().state,
        Readiness::Unavailable
    );
    assert_eq!(
        env.store
            .configuration(&denied.id)
            .unwrap()
            .reason
            .as_deref(),
        Some("container workspace is not writable")
    );
    let failed = env.store.instance(&id.to_string()).unwrap();
    assert!(!failed.prepared);
    assert!(failed.stopped);
    env.docker
        .delete(failed.handle.as_ref().unwrap())
        .await
        .unwrap();

    let mut broken = env.clone();
    broken.docker = Docker::new("unavailable-docker-example".into());
    assert!(broken.prepare(&config.id, Uuid::new_v4()).await.is_err());
    assert!(
        env.store
            .configuration(&config.id)
            .unwrap()
            .reason
            .unwrap()
            .contains("registry login")
    );
    drop(broken);
    drop(env);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "requires Docker and packaged static worker"]
async fn docker_windows_git_worktree_reaches_linux_worker_with_links_and_history() {
    round_trip_with_retry(
        "alpine:3.22",
        StartupMode::WorkerIdle,
        None,
        None,
        Some(PreflightRetry::WindowsSnapshot),
    )
    .await;
}
