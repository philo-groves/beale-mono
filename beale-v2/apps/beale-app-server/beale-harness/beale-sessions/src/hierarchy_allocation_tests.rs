use super::*;
use beale_contracts::{swarm_allocation::*, swarm_container::*};
use beale_fleet::{
    allocation::{Allocator, CapacityFuture, CapacitySource},
    docker::*,
};
use beale_storage::{fleet_allocation::AllocationStore, swarm_state::SwarmStore};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
struct OneSlot;
impl CapacitySource for OneSlot {
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
#[tokio::test]
#[ignore = "requires Docker, portable worker runtime, and synthetic host gateway"]
async fn docker_one_slot_deputy_yields_worker_completes_and_parent_resumes() {
    one_slot(false).await;
}
#[tokio::test]
#[ignore = "requires Docker, portable worker runtime, and synthetic host gateway"]
async fn docker_accepted_child_queues_when_competitor_takes_yielded_slot() {
    one_slot(true).await;
}
struct Contention {
    store: AllocationStore,
    injected: AtomicBool,
    queued: Arc<AtomicBool>,
}
impl CapacitySource for Contention {
    fn measure<'a>(
        &'a self,
        env: &'a DockerEnvironments,
        allocations: &'a [Allocation],
    ) -> CapacityFuture<'a> {
        Box::pin(async move {
            let capacity = OneSlot.measure(env, allocations).await?;
            if let Some(parent) = allocations.iter().find(|a| {
                a.request.domain == "docker"
                    && a.execution.is_some()
                    && a.phase == AllocationPhase::Retained
            }) && self
                .store
                .environment(&parent.request.instance)?
                .checkpoint_confirmed
                && !self.injected.swap(true, Ordering::SeqCst)
            {
                let mut contender = parent.request.clone();
                contender.id = "competitor-example".into();
                contender.scan = "competing-scan-example".into();
                contender.agent = "competing-agent-example".into();
                contender.instance = uuid::Uuid::new_v4().to_string();
                contender.release_parent = None;
                self.store.enqueue(contender, &capacity, Allocator::now())?;
                self.store
                    .admit_next(&capacity, Allocator::now())?
                    .expect("competitor acquired released slot");
            }
            if self.injected.load(Ordering::SeqCst)
                && allocations.iter().any(|a| {
                    a.request.domain == "docker"
                        && a.request.id != "competitor-example"
                        && a.phase == AllocationPhase::Queued
                })
                && !self.queued.swap(true, Ordering::SeqCst)
            {
                self.store.confirm_stopped("competitor-example", true)?;
                self.store.cancel_queued("competitor-example")?;
            }
            Ok(capacity)
        })
    }
}
async fn one_slot(contention: bool) {
    let leader_spawn = Arc::new(AtomicBool::new(false));
    let deputy_spawn = Arc::new(AtomicBool::new(false));
    let deputy_calls = Arc::new(AtomicUsize::new(0));
    let worker_calls = Arc::new(AtomicUsize::new(0));
    let (ls, ds, dc, wc) = (
        leader_spawn.clone(),
        deputy_spawn.clone(),
        deputy_calls.clone(),
        worker_calls.clone(),
    );
    let provider=Router::new().route("/v1/chat/completions",axum::routing::post(move |axum::Json(body):axum::Json<Value>|{
        let(ls,ds,dc,wc)=(ls.clone(),ds.clone(),dc.clone(),wc.clone());async move {
            let system=body["messages"][0]["content"].as_str().unwrap();
            let spawn=|profile:&str|json!({"tool_calls":[{"index":0,"id":format!("spawn-{profile}"),"type":"function","function":{"name":"swarm_spawn_subagent","arguments":json!({"profile":{"id":profile,"revision":1},"task":{"question":"Inspect synthetic example fixture","completion_criteria":["Return a synthetic result"],"evidence_refs":[]},"independent":false}).to_string()}}]});
            let delta=if system.contains("host-side reviewer") {json!({"content":json!({"decision":"approved","reason":"Synthetic managerial delegation"}).to_string()})}
            else if system.contains("Role: Worker") {wc.fetch_add(1,Ordering::SeqCst);json!({"content":"worker result example"})}
            else if system.contains("Role: Deputy") {dc.fetch_add(1,Ordering::SeqCst);if !ds.swap(true,Ordering::SeqCst){spawn("mapper")}else{json!({"content":"Deputy consumed the synthetic worker result"})}}
            else if !ls.swap(true,Ordering::SeqCst){spawn("research_deputy")}else{json!({"content":"Coordinator accepted synthetic progress"})};
            let chunk=json!({"choices":[{"delta":delta}]}); ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
    let (root, mut state, mut request, provider) = regression_session(provider).await;
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    state.fleet_origin = Some(origin.clone());
    let gateway = std::env::var("BEALE_DOCKER_TEST_GATEWAY").expect("synthetic gateway required");
    assert!(gateway.parse::<std::net::IpAddr>().is_ok());
    let env = DockerEnvironments::new(
        Docker::new(
            std::env::var_os("BEALE_DOCKER_CLI")
                .unwrap_or_else(|| "docker".into())
                .into(),
        ),
        DockerStore::open(state.db.clone()).unwrap(),
        root.join("fleet"),
        Some(std::env::var_os("BEALE_CONTAINER_RUNTIME").unwrap().into()),
    )
    .unwrap();
    env.store
        .save_configuration(ImageConfiguration {
            version: 1,
            id: "image-example".into(),
            revision: 1,
            image: "alpine:3.22".into(),
            architecture: LinuxArchitecture::Amd64,
            options: ContainerOptions {
                startup: StartupMode::ImageDefault,
                command: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    format!("printf '{gateway} localhost\\n' > /etc/hosts; exec tail -f /dev/null"),
                ],
                ..Default::default()
            },
        })
        .unwrap();
    let queued = Arc::new(AtomicBool::new(false));
    let capacity: Arc<dyn CapacitySource> = if contention {
        Arc::new(Contention {
            store: AllocationStore::open(state.db.clone()).unwrap(),
            injected: AtomicBool::new(false),
            queued: queued.clone(),
        })
    } else {
        Arc::new(OneSlot)
    };
    let allocator = Allocator::new(state.db.clone(), env.clone(), capacity).unwrap();
    let token = "a".repeat(64);
    beale_fleet::FleetCoordinationStore::open(state.db.clone())
        .unwrap()
        .save_local_endpoint(&origin, &token)
        .unwrap();
    let backend = Arc::new(crate::distributed::peer_host::AssignedWorkerBackend::new(
        root.join("fleet"),
        Arc::new(crate::distributed::docker::AllocatedDockerBackend(
            allocator.clone(),
        )),
    ));
    let app = crate::distributed::router()
        .with_state(state.clone())
        .merge(beale_fleet::worker_host::router(
            token.clone(),
            state.db.clone(),
            root.join("fleet"),
            backend.clone(),
        ))
        .merge(beale_fleet::allocation::http::router(
            token,
            allocator.clone(),
        ));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let maintenance = allocator.start_maintenance();
    let recovery =
        beale_fleet::worker_host::start_recovery(state.db.clone(), root.join("fleet"), backend);
    request.swarm_destinations = vec![beale_contracts::v1::SwarmDestination {
        host_id: env.store.host().into(),
        configuration_id: "image-example".into(),
    }];
    let detail = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        crate::hierarchy::run(state.clone(), request, None, None),
    )
    .await
    .unwrap()
    .unwrap();
    let saved = SwarmStore::open(state.db.clone())
        .unwrap()
        .load(&detail.summary.unwrap().id, true)
        .unwrap();
    assert_eq!(worker_calls.load(Ordering::SeqCst), 1);
    assert_eq!(queued.load(Ordering::SeqCst), contention);
    assert!(deputy_calls.load(Ordering::SeqCst) >= 2);
    let deputy = saved
        .agents
        .iter()
        .find(|a| a.summary.role == beale_contracts::swarms::Role::Deputy)
        .unwrap();
    assert_eq!(
        saved
            .attempts
            .iter()
            .filter(|a| a.agent_id == deputy.summary.id)
            .count(),
        2
    );
    assert!(saved.ended_at_ms.is_some());
    let rows = AllocationStore::open(state.db.clone())
        .unwrap()
        .allocations()
        .unwrap();
    assert_eq!(
        rows.iter()
            .filter(|a| a.request.domain == "docker" && a.request.id != "competitor-example")
            .count(),
        3
    );
    assert!(
        rows.iter()
            .filter(|a| a.request.id != "competitor-example")
            .all(|a| if a.request.domain == "docker" {
                a.phase == AllocationPhase::Retained
            } else {
                a.request.domain == "canonical-storage" && a.phase == AllocationPhase::Canceled
            })
    );
    assert_eq!(
        allocator.store.environments().unwrap().len(),
        if contention { 3 } else { 2 }
    );
    for environment in allocator.store.environments().unwrap() {
        assert!(environment.checkpoint_confirmed && environment.pending_outputs.is_empty());
        allocator
            .store
            .retention(
                &environment.instance,
                environment.generation,
                RetentionAction::ClaimDelete { now: true },
                Allocator::now(),
            )
            .unwrap();
    }
    allocator.cleanup().await.unwrap();
    maintenance.abort();
    recovery.abort();
    server.abort();
    provider.abort();
    let _ = maintenance.await;
    let _ = recovery.await;
    let _ = server.await;
    let _ = provider.await;
    drop(allocator);
    drop(env);
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}
