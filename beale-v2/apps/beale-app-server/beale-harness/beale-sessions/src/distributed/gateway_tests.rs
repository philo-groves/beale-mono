use super::*;
use crate::{SessionState, tool_runtime::unix_ms};
use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::Response,
};
use beale_contracts::{swarm_local::*, swarm_worker::WorkerReply, swarms::*};
use beale_storage::{HostDatabase, WorkspaceRow, swarm_state::*};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;

#[path = "coordinator_tests.rs"]
mod coordinators;
#[path = "docker_tests.rs"]
mod docker_acceptance;
#[path = "late_evidence_tests.rs"]
mod late_evidence;
#[path = "partition_tests.rs"]
mod partitions;
#[path = "restart_tests.rs"]
mod restarts;

struct Fixture {
    workspace_id: String,
    root: PathBuf,
    state: SessionState,
    store: SwarmStore,
    leader: String,
    deputy: String,
    attempt: ExecutionAttempt,
    credential: WorkerCredential,
    url: String,
    server: tokio::task::JoinHandle<()>,
    lose_response: Arc<AtomicBool>,
    block_resource_chunks: Arc<AtomicBool>,
}
impl Fixture {
    fn allocation_snapshot(&self) {
        beale_storage::swarm_files::ResearchFiles::open(
            self.state.db.clone(),
            self.state.workspaces.root(),
        )
        .unwrap()
        .import_snapshot(
            "scan-example",
            &beale_contracts::swarm_files::Manifest {
                version: 1,
                entries: vec![],
                repositories: vec![],
            },
        )
        .unwrap();
    }
    async fn new() -> Self {
        Self::with_tools(true).await
    }
    async fn with_tools(enable_tools: bool) -> Self {
        Self::with_shell(
            enable_tools,
            beale_contracts::swarm_worker::WorkerShell::native(),
            false,
        )
        .await
    }
    async fn with_shell(
        enable_tools: bool,
        shell: beale_contracts::swarm_worker::WorkerShell,
        command: bool,
    ) -> Self {
        Self::with_workspace(enable_tools, shell, command, "workspace-example").await
    }
    async fn with_workspace(
        enable_tools: bool,
        shell: beale_contracts::swarm_worker::WorkerShell,
        command: bool,
        workspace_id: &str,
    ) -> Self {
        Self::with_selection(enable_tools, shell, command, workspace_id, None).await
    }
    async fn with_selection(
        enable_tools: bool,
        shell: beale_contracts::swarm_worker::WorkerShell,
        command: bool,
        workspace_id: &str,
        destination: Option<Destination>,
    ) -> Self {
        Self::with_definition(
            enable_tools,
            shell,
            command,
            workspace_id,
            destination,
            beale_swarms::BASIC_SWARM.snapshot.clone(),
            "research_deputy",
        )
        .await
    }
    async fn with_definition(
        enable_tools: bool,
        shell: beale_contracts::swarm_worker::WorkerShell,
        command: bool,
        workspace_id: &str,
        destination: Option<Destination>,
        snapshot: DefinitionSnapshot,
        deputy_profile: &str,
    ) -> Self {
        let root =
            std::env::temp_dir().join(format!("beale-worker-gateway-example-{}", Uuid::new_v4()));
        let workspaces = beale_workspaces::WorkspaceStore::at(root.join("host")).unwrap();
        let state = crate::build_state("host-control-example".into(), workspaces.clone()).unwrap();
        state
            .db
            .replace_workspaces(&[WorkspaceRow {
                id: workspace_id.into(),
                name: "Example".into(),
                directory: root.join("workspace").to_string_lossy().into(),
            }])
            .unwrap();
        for rule in beale_safety::rules(&state.db).unwrap() {
            state
                .db
                .save_permission_rule(&rule.id, &rule.title, &rule.instruction, false)
                .unwrap();
        }
        let store = SwarmStore::open(state.db.clone()).unwrap();
        let initialize = if destination.is_some() {
            SwarmStore::peer_initialize
        } else {
            SwarmStore::local_initialize
        };
        let profiles = snapshot.profiles.clone();
        let saved = store
            .create_scan(
                ScanConfig {
                    version: VERSION,
                    scan_id: "scan-example".into(),
                    workspace_id: workspace_id.into(),
                    mode: ScanMode::Swarm {
                        snapshot: Box::new(snapshot),
                    },
                    authorized_scope: "Only synthetic example.test fixtures".into(),
                    enabled_tools: if enable_tools {
                        let mut tools = vec![
                            "read".into(),
                            "write".into(),
                            "plugin_call".into(),
                            #[cfg(feature = "webfetch")]
                            "webfetch".into(),
                            #[cfg(feature = "findings")]
                            "findings_create".into(),
                            #[cfg(feature = "findings")]
                            "findings_read".into(),
                            #[cfg(feature = "findings")]
                            "findings_query".into(),
                            #[cfg(feature = "findings")]
                            "findings_update".into(),
                            #[cfg(feature = "findings")]
                            "findings_verify".into(),
                            #[cfg(feature = "surfaces")]
                            "surface_query".into(),
                            #[cfg(feature = "surfaces")]
                            "surface_read".into(),
                            #[cfg(feature = "surfaces")]
                            "surface_save".into(),
                            #[cfg(feature = "surfaces")]
                            "surface_threat_model_edit".into(),
                            #[cfg(feature = "agent-feedback")]
                            "agent_feedback".into(),
                            #[cfg(feature = "memories")]
                            "memory_query".into(),
                            #[cfg(feature = "memories")]
                            "memory_read".into(),
                            #[cfg(feature = "memories")]
                            "memory_graph".into(),
                            #[cfg(feature = "memories")]
                            "memory_session_query".into(),
                            #[cfg(feature = "runbooks")]
                            "runbook_create".into(),
                            #[cfg(feature = "runbooks")]
                            "runbook_edit".into(),
                            #[cfg(feature = "runbooks")]
                            "runbook_get".into(),
                            #[cfg(feature = "runbooks")]
                            "runbook_query".into(),
                            #[cfg(feature = "runbooks")]
                            "runbook_status".into(),
                            #[cfg(feature = "runbooks")]
                            "runbook_run".into(),
                            "swarm_spawn_subagent".into(),
                            "swarm_list_subagents".into(),
                            "swarm_message_parent".into(),
                            "swarm_handoff".into(),
                            "swarm_message_subagent".into(),
                            "swarm_stop_subagent".into(),
                            "swarm_materialize_artifact".into(),
                        ];
                        if command {
                            tools.push(beale_tools::COMMAND_TOOL_NAME.into());
                        }
                        tools
                    } else {
                        vec![]
                    },
                    provider_routes: profiles
                        .iter()
                        .enumerate()
                        .map(|(index, profile)| ProviderRoute {
                            id: format!("{}-{index}", profile.profile.id),
                            provider_id: format!(
                                "provider-{}",
                                beale_contracts::v1::ProviderId::Custom as i32
                            ),
                            model: "custom-example::example".into(),
                            budget_group_id: "provider-example".into(),
                        })
                        .collect(),
                    destinations: destination.into_iter().collect(),
                    pacing: vec![],
                    retention: RetentionPolicy::default(),
                },
                true,
                true,
            )
            .unwrap();
        let leader = saved.agents[0].summary.id.clone();
        initialize(
            &store,
            "scan-example",
            LocalSettings {
                models: profiles
                    .iter()
                    .map(|profile| LocalModel {
                        profile: profile.profile.id.clone(),
                        provider: beale_contracts::v1::ProviderId::Custom as i32,
                        method: beale_contracts::v1::AuthMethod::ApiKey as i32,
                        model: "custom-example::example".into(),
                    })
                    .collect(),
                reasoning_effort: 0,
            },
            "Coordinate the synthetic fixture",
        )
        .unwrap();
        let leader_turn = store
            .local_claim("scan-example", &leader, unix_ms(), 64)
            .unwrap()
            .unwrap();
        let spawned = store
            .local_dispatch(
                &leader_turn,
                "initial-deputy",
                SwarmCommand::Spawn(Spawn {
                    handoff_id: None,
                    profile: profiles
                        .iter()
                        .find(|p| p.role == Role::Deputy && p.profile.id == deputy_profile)
                        .unwrap()
                        .profile
                        .clone(),
                    task: task(),
                    independent: false,
                }),
                Admission::Ready,
            )
            .unwrap();
        let deputy = spawned["child_id"].as_str().unwrap().to_owned();
        store
            .local_finish(&leader_turn, json!({}), "Waiting for research", false)
            .unwrap();
        let turn = store
            .local_claim("scan-example", &deputy, unix_ms(), 64)
            .unwrap()
            .unwrap();
        let saved = store.load("scan-example", true).unwrap();
        let attempt = saved
            .attempts
            .iter()
            .find(|attempt| attempt.id == turn.attempt_id)
            .unwrap()
            .clone();
        let (_, credential) =
            prepare_worker(&state.db, &turn, shell, &registration(), unix_ms()).unwrap();
        let lose_response = Arc::new(AtomicBool::new(false));
        let fault = lose_response.clone();
        let block_resource_chunks = Arc::new(AtomicBool::new(false));
        let chunk_fault = block_resource_chunks.clone();
        let app = crate::router("host-control-example".into(), workspaces)
            .unwrap()
            .layer(middleware::from_fn(
                move |request: Request<Body>, next: Next| {
                    let fault = fault.clone();
                    let chunk_fault=chunk_fault.clone();
                    async move {
                        let response = next.run(request).await;
                        let response=if chunk_fault.load(Ordering::SeqCst) && response.status().is_success() {
                            let (parts,body)=response.into_parts();
                            let bytes=axum::body::to_bytes(body,MAX_DOCUMENT_BYTES+4096).await.unwrap();
                            let blocked=serde_json::from_slice::<WorkerReply>(&bytes).ok().and_then(|reply|match reply {
                                WorkerReply::Acknowledged {acknowledgement}=>serde_json::from_value::<beale_contracts::swarm_plugins::ResourceReply>(acknowledgement.result).ok(),_=>None,
                            }).is_some_and(|reply|matches!(reply.result,beale_contracts::swarm_plugins::ResourceResult::Chunk {offset,..} if offset>0));
                            if blocked {Response::builder().status(StatusCode::SERVICE_UNAVAILABLE).body(Body::empty()).unwrap()}
                            else {Response::from_parts(parts,Body::from(bytes))}
                        } else {response};
                        if response.status().is_success() && fault.swap(false, Ordering::SeqCst) {
                            Response::builder()
                                .status(StatusCode::SERVICE_UNAVAILABLE)
                                .body(Body::empty())
                                .unwrap()
                        } else {
                            response
                        }
                    }
                },
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            workspace_id: workspace_id.into(),
            root,
            state,
            store,
            leader,
            deputy,
            attempt,
            credential,
            url,
            server,
            lose_response,
            block_resource_chunks,
        }
    }
    fn request(&self, id: &str, operation: WorkerOperation) -> WorkerRequest {
        WorkerRequest {
            version: VERSION,
            workspace_id: self.workspace_id.clone(),
            scan_id: "scan-example".into(),
            agent_id: self.deputy.clone(),
            attempt_id: self.attempt.id.clone(),
            generation: self.attempt.generation,
            operation_id: id.into(),
            operation,
        }
    }
    fn client(&self) -> WorkerTransport {
        WorkerTransport::new(&self.url, self.credential.bearer()).unwrap()
    }
    async fn close(self) {
        self.server.abort();
        let _ = self.server.await;
        drop(self.store);
        drop(self.state);
        std::fs::remove_dir_all(self.root).unwrap();
    }
}
fn task() -> Task {
    Task {
        question: "Inspect a synthetic fixture".into(),
        completion_criteria: vec!["Return candidate evidence".into()],
        evidence_refs: vec![],
    }
}

#[tokio::test]
async fn assigned_loop_executes_in_private_workspace_and_finishes_without_releasing_ownership() {
    assigned_loop_write(false).await;
}

#[test]
#[ignore = "subprocess fixture invoked by assigned supervisor integration"]
fn assigned_worker_process_fixture() {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    let launch: WorkerLaunch = serde_json::from_slice(&bytes).unwrap();
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(worker_entry(launch))
        .unwrap();
}

#[tokio::test]
async fn owning_supervisor_accepts_real_worker_transcript_only_after_process_termination() {
    let f = Fixture::new().await;
    let (server, calls) = inference_provider(&f, false).await;
    let workspace = f.root.join("worker-workspace");
    std::fs::create_dir(&workspace).unwrap();
    let launch = WorkerLaunch {
        version: 1,
        identity: f.store.worker_bootstrap(&f.attempt.id).unwrap().identity,
        leader: f.url.clone(),
        bearer: f.credential.bearer().into(),
        workspace: workspace.clone(),
        protected_root: f.root.clone(),
        recovery: f.root.join("worker-recovery"),
    };
    let args = [
        "--ignored",
        "--exact",
        "distributed::gateway_tests::assigned_worker_process_fixture",
    ]
    .map(std::ffi::OsString::from);
    let mut process = WorkerProcess::spawn(&std::env::current_exe().unwrap(), &args, &workspace)
        .await
        .unwrap();
    let (_cancel, receiver) = tokio::sync::watch::channel(false);
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(15),
            supervise_assignment(&mut process, &launch, &f.store, receiver)
        )
        .await
        .unwrap()
        .unwrap(),
        WorkerReconciliation::TurnAccepted
    );
    assert!(process.confirm_stopped().unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.state.db.session(&f.deputy).unwrap().unwrap().turns.len(),
        1
    );
    let saved = f.store.load("scan-example", true).unwrap();
    assert_eq!(
        saved.agent(&f.deputy).unwrap().summary.state,
        AgentState::Completed
    );
    assert!(
        saved
            .attempts
            .iter()
            .any(|attempt| attempt.id == f.attempt.id && attempt.state == ExecutionState::Stopped)
    );
    assert!(
        f.client()
            .send(
                &f.request("late-heartbeat", WorkerOperation::Heartbeat {}),
                Duration::from_secs(5)
            )
            .await
            .is_err()
    );
    drop(process);
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn assigned_loop_retains_uncertain_effect_after_outcome_write_failure_without_reexecution() {
    assigned_loop_write(true).await;
}

async fn assigned_loop_write(fail_outcome: bool) {
    use beale_storage::worker_spool::{SpoolFaults, SpoolWrite};
    struct FailAfterWrite(std::path::PathBuf);
    impl SpoolFaults for FailAfterWrite {
        fn before_commit(&self, write: SpoolWrite) -> anyhow::Result<()> {
            anyhow::ensure!(
                write != SpoolWrite::Outcome || !self.0.exists(),
                "synthetic outcome disk full"
            );
            Ok(())
        }
    }
    let f = Fixture::new().await;
    let workspace = f.root.join("swarm-files/environments/worker-example");
    std::fs::create_dir_all(&workspace).unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = calls.clone();
    let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let calls = captured.clone();
        async move {
            assert_eq!(body["model"], "example");
            let call = calls.fetch_add(1, Ordering::SeqCst);
            let delta = if call == 0 {
                json!({"content":"Synthetic progress before the tool", "tool_calls":[{"index":0,"id":"write-example","type":"function","function":{"name":"write","arguments":json!({"path":"candidate.txt","content":"Synthetic candidate output"}).to_string()}}]})
            } else {
                assert!(body["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool" && m["tool_call_id"] == "write-example"));
                json!({"content":"Synthetic assignment complete"})
            };
            let chunk = json!({"choices":[{"delta":delta}],"usage":{"prompt_tokens":10,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":3}}});
            ([("content-type","text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
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
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let launch = || WorkerLaunch {
        version: 1,
        identity: beale_contracts::swarm_worker::ExecutionIdentity {
            workspace_id: "workspace-example".into(),
            scan_id: "scan-example".into(),
            agent_id: f.deputy.clone(),
            attempt_id: f.attempt.id.clone(),
            generation: f.attempt.generation,
        },
        leader: f.url.clone(),
        bearer: f.credential.bearer().into(),
        workspace: workspace.clone(),
        protected_root: f.root.clone(),
        recovery: f.root.join("worker-recovery"),
    };
    f.lose_response.store(true, Ordering::SeqCst);
    let faults: Option<Arc<dyn SpoolFaults>> = fail_outcome
        .then(|| Arc::new(FailAfterWrite(workspace.join("candidate.txt"))) as Arc<dyn SpoolFaults>);
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        super::worker::run_with_faults(&mut launch(), faults),
    )
    .await
    .unwrap();
    assert_eq!(result.is_err(), fail_outcome);
    assert_eq!(
        std::fs::read_to_string(workspace.join("candidate.txt")).unwrap(),
        "Synthetic candidate output"
    );
    let progress = f.store.worker_progress(&f.attempt.id).unwrap();
    assert_eq!(
        progress.answer.as_deref(),
        (!fail_outcome).then_some("Synthetic assignment complete")
    );
    assert!(progress.revision >= if fail_outcome { 2 } else { 4 });
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .attempts
            .iter()
            .any(|a| a.id == f.attempt.id && a.state == ExecutionState::Running)
    );
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .assignments
            .iter()
            .any(|a| a.child_id == f.deputy && a.state == AssignmentState::Assigned)
    );
    std::fs::write(workspace.join("candidate.txt"), "Changed after completion").unwrap();
    assert_eq!(run_assigned(launch()).await.is_err(), fail_outcome);
    assert_eq!(
        std::fs::read_to_string(workspace.join("candidate.txt")).unwrap(),
        "Changed after completion"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        if fail_outcome { 1 } else { 2 }
    );
    assert!(!f.root.join("worker-recovery/state.sqlite").exists());
    let disposition = if fail_outcome {
        WorkerDisposition::Failed
    } else {
        WorkerDisposition::Completed
    };
    assert_eq!(
        f.store
            .reconcile_worker(&f.attempt.id, false, disposition, unix_ms())
            .unwrap(),
        WorkerReconciliation::TerminationUnconfirmed
    );
    assert!(
        f.store
            .local_claim("scan-example", &f.deputy, unix_ms(), 64)
            .unwrap()
            .is_none()
    );
    assert!(f.state.db.session(&f.deputy).unwrap().is_none());
    f.state.db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_worker_transcript BEFORE INSERT ON session_turns BEGIN SELECT RAISE(ABORT, 'synthetic transcript full'); END;")?; Ok(()) }).unwrap();
    assert!(
        f.store
            .reconcile_worker(&f.attempt.id, true, disposition, unix_ms())
            .is_err()
    );
    assert!(f.state.db.session(&f.deputy).unwrap().is_none());
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .attempts
            .iter()
            .any(|attempt| attempt.id == f.attempt.id
                && attempt.state == ExecutionState::TerminationUnconfirmed)
    );
    f.state
        .db
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER fail_worker_transcript;")?;
            Ok(())
        })
        .unwrap();
    // A failure in the final release receipt must roll back the transcript,
    // parent notification, assignment change, and execution release together.
    f.state.db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_supervisor_receipt BEFORE INSERT ON swarm_receipts WHEN NEW.actor='supervisor' BEGIN SELECT RAISE(ABORT, 'synthetic supervisor receipt full'); END;")?; Ok(()) }).unwrap();
    assert!(
        f.store
            .reconcile_worker(&f.attempt.id, true, disposition, unix_ms())
            .is_err()
    );
    assert!(f.state.db.session(&f.deputy).unwrap().is_none());
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .assignments
            .iter()
            .any(|assignment| assignment.child_id == f.deputy
                && assignment.state == AssignmentState::Assigned)
    );
    f.state
        .db
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER fail_supervisor_receipt;")?;
            Ok(())
        })
        .unwrap();
    let expected = if fail_outcome {
        WorkerReconciliation::BranchStopped
    } else {
        WorkerReconciliation::TurnAccepted
    };
    assert_eq!(
        f.store
            .reconcile_worker(&f.attempt.id, true, disposition, unix_ms())
            .unwrap(),
        expected
    );
    assert_eq!(
        f.store
            .reconcile_worker(&f.attempt.id, true, disposition, unix_ms())
            .unwrap(),
        expected
    );
    assert_eq!(
        f.state.db.session(&f.deputy).unwrap().unwrap().turns.len(),
        1
    );
    assert!(progress.active_elapsed_ms > 0);
    assert_eq!(
        f.state.db.session(&f.deputy).unwrap().unwrap().turns[0].duration_ms,
        i64::try_from(progress.active_elapsed_ms).unwrap()
    );
    let usage = f.state.db.session_model_usage(&f.deputy).unwrap();
    assert_eq!(usage.len(), if fail_outcome { 1 } else { 2 });
    assert!(usage.iter().all(|item| item.agent_id == f.deputy
        && item.role == progress.turn.profile
        && item.input_tokens == Some(10)
        && item.output_tokens == Some(5)
        && item.cache_read_tokens == Some(3)
        && item.observed_at_ms > 0));
    let commentary = f.state.db.commentary(&f.deputy).unwrap();
    assert_eq!(commentary.len(), 1);
    assert_eq!(commentary[0].text, "Synthetic progress before the tool");
    assert_eq!(commentary[0].after_tool_count, 0);
    let saved = f.store.load("scan-example", true).unwrap();
    assert!(
        saved
            .attempts
            .iter()
            .any(|attempt| attempt.id == f.attempt.id && attempt.state == ExecutionState::Stopped)
    );
    assert_eq!(
        saved.agent(&f.deputy).unwrap().summary.state,
        if fail_outcome {
            AgentState::Blocked
        } else {
            AgentState::Completed
        }
    );
    assert_ne!(
        saved.agent(&f.leader).unwrap().summary.state,
        AgentState::Stopped
    );
    assert_eq!(
        f.store
            .local_state("scan-example")
            .unwrap()
            .inbox
            .iter()
            .filter(|message| message.from == f.deputy && message.to == f.leader)
            .count(),
        1
    );
    if !fail_outcome {
        let tools = f.state.db.tool_summaries(&f.deputy).unwrap();
        assert_eq!(tools.len(), 1);
        let tool = f
            .state
            .db
            .tool_call(&f.deputy, &tools[0].id)
            .unwrap()
            .unwrap();
        assert!(tool.created_at_ms > 0);
        assert_eq!(
            serde_json::from_str::<Value>(&tool.arguments).unwrap()["path"],
            "candidate.txt"
        );
        f.store
            .local_message(
                "scan-example",
                "related-follow-up",
                &f.deputy,
                "Related synthetic work",
            )
            .unwrap();
        let next = f
            .store
            .local_claim("scan-example", &f.deputy, unix_ms(), 64)
            .unwrap()
            .unwrap();
        assert_eq!(next.generation, f.attempt.generation + 1);
        assert_eq!(
            f.store
                .reconcile_worker(&f.attempt.id, true, disposition, unix_ms())
                .unwrap(),
            WorkerReconciliation::TurnAccepted
        );
        assert!(
            f.store
                .load("scan-example", true)
                .unwrap()
                .attempts
                .iter()
                .any(|attempt| attempt.id == next.attempt_id
                    && attempt.state == ExecutionState::Running)
        );
    }
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn reconciliation_preserves_host_reasoning_and_usage_order_and_rejects_activity_races() {
    let f = Fixture::new().await;
    let complete = |id: &str, result: Value| {
        let request = inference_request(&f, id);
        let owner = f
            .store
            .try_own_inference(f.credential.bearer(), &request, unix_ms())
            .unwrap()
            .unwrap();
        let InferenceClaim::Dispatch(claim) =
            owner.claim(f.credential.bearer(), unix_ms()).unwrap()
        else {
            panic!("expected claim");
        };
        let ack = owner
            .complete(f.credential.bearer(), &claim, result.clone(), unix_ms())
            .unwrap();
        assert_eq!(
            owner
                .complete(f.credential.bearer(), &claim, result, unix_ms())
                .unwrap(),
            ack
        );
    };
    complete(
        "before-tool-example",
        json!({"status":"completed","text":"Synthetic commentary",
        "calls":[{"id":"tool-example","name":"read","arguments":"{}"}],
        "reasoning_summaries":["", "Synthetic first summary"],"continuation":[],"response_id":null,
        "usage":{"input_tokens":20,"output_tokens":5,"cache_read_tokens":4}}),
    );
    let event = f.request("tool-event-example", WorkerOperation::Event { event_id:"tool-event-example".into(),
        payload:json!({"kind":"tool_outcome","tool":"read","arguments":{"path":"example.txt"},"result":{"output":"Synthetic observation","is_error":false}}) });
    let accepted = f
        .client()
        .send(&event, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        f.client()
            .send(&event, Duration::from_secs(5))
            .await
            .unwrap(),
        accepted
    );
    complete(
        "unavailable-example",
        json!({"status":"unavailable","retryable":true}),
    );
    complete(
        "after-tool-example",
        json!({"status":"completed","text":"Synthetic final answer","calls":[],
        "reasoning_summaries":["Synthetic second summary"],"continuation":[],"response_id":null,
        "usage":{"input_tokens":30,"output_tokens":7,"cache_read_tokens":null}}),
    );
    let pending = inference_request(&f, "pending-accounting-example");
    let owner = f
        .store
        .try_own_inference(f.credential.bearer(), &pending, unix_ms())
        .unwrap()
        .unwrap();
    assert!(matches!(
        owner.claim(f.credential.bearer(), unix_ms()).unwrap(),
        InferenceClaim::Dispatch(_)
    ));
    drop(owner);
    // Guest events are inert: they cannot manufacture provider accounting or summaries.
    f.client().send(&f.request("spoof-usage-example", WorkerOperation::Event {
        event_id:"spoof-usage-example".into(), payload:json!({"kind":"inference-completed","reasoning_summaries":["Forged summary"],"usage":{"input_tokens":999}}),
    }), Duration::from_secs(5)).await.unwrap();
    for (id, operation) in [
        (
            "final-checkpoint-example",
            WorkerOperation::Checkpoint {
                active_elapsed_ms: 0,
                revision: 1,
                context: json!({"input":[],"phase":{"phase":"done","answer":"Synthetic final answer"}}),
            },
        ),
        (
            "final-answer-example",
            WorkerOperation::Finish {
                revision: 1,
                answer: "Synthetic final answer".into(),
            },
        ),
    ] {
        f.client()
            .send(&f.request(id, operation), Duration::from_secs(5))
            .await
            .unwrap();
    }
    f.state.db.with_connection(|db| { db.execute_batch("CREATE TRIGGER late_inference_example BEFORE INSERT ON session_turns BEGIN UPDATE swarm_inference_activity SET completed_at_ms=1 WHERE operation_id='pending-accounting-example'; END;")?; Ok(()) }).unwrap();
    assert!(
        f.store
            .reconcile_worker(&f.attempt.id, true, WorkerDisposition::Completed, unix_ms())
            .unwrap_err()
            .to_string()
            .contains("inference changed")
    );
    assert!(f.state.db.session(&f.deputy).unwrap().is_none());
    let scan = f.store.load("scan-example", true).unwrap();
    let late_event = json!({"id":"late-event-example","agent_id":f.deputy,"sequence":scan.events.len()+1,"kind":"worker-event",
        "payload":{"attempt_id":f.attempt.id,"generation":f.attempt.generation,"observed_at_ms":unix_ms(),"event":{"kind":"warning","message":"Synthetic late event"}}});
    f.state.db.with_connection(|db| {
        db.execute_batch("DROP TRIGGER late_inference_example; CREATE TABLE late_event_example(payload TEXT NOT NULL);")?;
        db.execute("INSERT INTO late_event_example VALUES(?1)",[late_event.to_string()])?;
        db.execute_batch("CREATE TRIGGER late_worker_event_example BEFORE INSERT ON session_turns BEGIN UPDATE swarm_scans SET document=json_insert(document,'$.events[#]',json((SELECT payload FROM late_event_example))) WHERE id='scan-example'; END;")?;
        Ok(())
    }).unwrap();
    assert!(
        f.store
            .reconcile_worker(&f.attempt.id, true, WorkerDisposition::Completed, unix_ms())
            .unwrap_err()
            .to_string()
            .contains("events changed")
    );
    assert!(f.state.db.session(&f.deputy).unwrap().is_none());
    f.state.db.with_connection(|db| { db.execute_batch("DROP TRIGGER late_worker_event_example; DROP TABLE late_event_example; CREATE TRIGGER fail_usage_example BEFORE INSERT ON session_model_usage BEGIN SELECT RAISE(ABORT, 'synthetic usage full'); END;")?; Ok(()) }).unwrap();
    assert!(
        f.store
            .reconcile_worker(&f.attempt.id, true, WorkerDisposition::Completed, unix_ms())
            .is_err()
    );
    assert!(
        f.state
            .db
            .reasoning_summaries(&f.deputy)
            .unwrap()
            .is_empty()
    );
    assert!(f.state.db.commentary(&f.deputy).unwrap().is_empty());
    assert!(
        f.state
            .db
            .session_model_usage(&f.deputy)
            .unwrap()
            .is_empty()
    );
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_usage_example;")?;
            Ok(())
        })
        .unwrap();
    let reopened = SwarmStore::open(HostDatabase::open(&f.root.join("host")).unwrap()).unwrap();
    for _ in 0..2 {
        assert_eq!(
            reopened
                .reconcile_worker(&f.attempt.id, true, WorkerDisposition::Completed, unix_ms())
                .unwrap(),
            WorkerReconciliation::TurnAccepted
        );
    }
    let reasoning = f.state.db.reasoning_summaries(&f.deputy).unwrap();
    assert_eq!(
        reasoning
            .iter()
            .map(|row| (row.text.as_str(), row.after_tool_count))
            .collect::<Vec<_>>(),
        vec![
            ("Synthetic first summary", 0),
            ("Synthetic second summary", 1)
        ]
    );
    let commentary = f.state.db.commentary(&f.deputy).unwrap();
    assert_eq!(commentary.len(), 1);
    assert_eq!(
        (commentary[0].text.as_str(), commentary[0].after_tool_count),
        ("Synthetic commentary", 0)
    );
    let usage = f.state.db.session_model_usage(&f.deputy).unwrap();
    assert_eq!(usage.len(), 4);
    assert_eq!(
        usage.iter().map(|row| row.input_tokens).collect::<Vec<_>>(),
        vec![Some(20), None, Some(30), None]
    );
    let totals = f.state.db.session_usage_totals(&f.deputy).unwrap();
    assert_eq!(
        (
            totals.input_tokens,
            totals.output_tokens,
            totals.total_calls,
            totals.measured_calls,
            totals.cache_read_tokens
        ),
        (50, 12, 4, 2, 4)
    );
    assert!(
        usage
            .iter()
            .all(|row| row.agent_id == f.deputy && row.role == "research_deputy")
    );
    drop(reopened);
    f.close().await;
}

#[tokio::test]
async fn worker_bootstrap_and_candidate_checkpoints_cannot_forge_current_assignment_completion() {
    let f = Fixture::new().await;
    let request = f.request("bootstrap-example", WorkerOperation::Bootstrap {});
    let bootstrap = acknowledged(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    assert_eq!(bootstrap["identity"]["agent_id"], f.deputy);
    assert!(!bootstrap.to_string().contains(f.credential.bearer()));
    let progress = f.store.worker_progress(&f.attempt.id).unwrap();
    assert!(
        f.store
            .prepare_worker(
                &progress.turn,
                &serde_json::from_value(bootstrap).unwrap(),
                &registration(),
                unix_ms()
            )
            .is_err()
    );
    let early = f.request(
        "finish-too-early",
        WorkerOperation::Finish {
            revision: 1,
            answer: "Uncommitted completion".into(),
        },
    );
    assert!(
        f.client()
            .send(&early, Duration::from_secs(5))
            .await
            .is_err()
    );
    let checkpoint = f.request(
        "checkpoint-example",
        WorkerOperation::Checkpoint {
            active_elapsed_ms: 0,
            revision: 2,
            context: json!({"synthetic":"retained"}),
        },
    );
    f.client()
        .send(&checkpoint, Duration::from_secs(5))
        .await
        .unwrap();
    let changed = f.request(
        "changed-checkpoint-example",
        WorkerOperation::Checkpoint {
            active_elapsed_ms: 0,
            revision: 2,
            context: json!({"synthetic":"changed"}),
        },
    );
    assert!(
        f.client()
            .send(&changed, Duration::from_secs(5))
            .await
            .is_err()
    );
    let done = f.request(
        "finish-example",
        WorkerOperation::Finish {
            revision: 2,
            answer: "Candidate completion".into(),
        },
    );
    let result = acknowledged(
        f.client()
            .send(&done, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    assert_eq!(result["termination_confirmed"], false);
    let checkpoint = f.request(
        "late-checkpoint-example",
        WorkerOperation::Checkpoint {
            active_elapsed_ms: 0,
            revision: 3,
            context: json!({}),
        },
    );
    assert!(
        f.client()
            .send(&checkpoint, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .attempts
            .iter()
            .any(|a| a.id == f.attempt.id && a.state == ExecutionState::Running)
    );
    f.close().await;
}

#[tokio::test]
async fn supervisor_reconciliation_preserves_cancellation_and_terminal_leader_failure_stops_descendants()
 {
    use beale_contracts::swarm_worker::WorkerShell;
    let f = Fixture::new().await;
    f.store.local_stop("scan-example", &f.deputy).unwrap();
    assert_eq!(
        f.store
            .reconcile_worker(&f.attempt.id, false, WorkerDisposition::Failed, unix_ms())
            .unwrap(),
        WorkerReconciliation::TerminationUnconfirmed
    );
    assert_eq!(
        f.store
            .reconcile_worker(&f.attempt.id, true, WorkerDisposition::Failed, unix_ms())
            .unwrap(),
        WorkerReconciliation::BranchStopped
    );
    assert_eq!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .agent(&f.deputy)
            .unwrap()
            .summary
            .state,
        AgentState::Stopped
    );
    assert!(
        f.store
            .local_claim("scan-example", &f.deputy, unix_ms(), 64)
            .unwrap()
            .is_none()
    );
    f.close().await;

    let f = Fixture::new().await;
    f.store
        .local_message(
            "scan-example",
            "leader-follow-up",
            &f.leader,
            "Coordinate synthetic follow-up",
        )
        .unwrap();
    let turn = f
        .store
        .local_claim("scan-example", &f.leader, unix_ms(), 64)
        .unwrap()
        .unwrap();
    prepare_worker(
        &f.state.db,
        &turn,
        WorkerShell::native(),
        &registration(),
        unix_ms(),
    )
    .unwrap();
    assert_eq!(
        f.store
            .reconcile_worker(&turn.attempt_id, true, WorkerDisposition::Failed, unix_ms())
            .unwrap(),
        WorkerReconciliation::BranchStopped
    );
    let saved = f.store.load("scan-example", true).unwrap();
    assert_eq!(
        saved.agent(&f.leader).unwrap().summary.state,
        AgentState::Stopped
    );
    assert!(
        saved
            .attempts
            .iter()
            .any(|attempt| attempt.id == f.attempt.id && attempt.state == ExecutionState::Running)
    );
    assert!(
        f.store
            .local_state("scan-example")
            .unwrap()
            .controls
            .iter()
            .all(|control| control.stop_requested)
    );
    assert!(
        f.client()
            .send(
                &f.request("after-leader-failure", WorkerOperation::Heartbeat {}),
                Duration::from_secs(5)
            )
            .await
            .is_err()
    );
    f.close().await;
}

#[tokio::test]
async fn worker_recovery_refuses_unknown_effects_and_failed_checkpoints_before_inference() {
    use beale_contracts::swarm_worker::{ExecutionIdentity, ToolInvocation, WorkRequest};
    use beale_storage::worker_spool::{SpoolFaults, SpoolWrite, WorkerSpool};
    struct FailCheckpoint;
    impl SpoolFaults for FailCheckpoint {
        fn before_commit(&self, write: SpoolWrite) -> anyhow::Result<()> {
            anyhow::ensure!(
                write != SpoolWrite::Checkpoint,
                "synthetic checkpoint disk full"
            );
            Ok(())
        }
    }
    for unknown in [false, true] {
        let f = Fixture::new().await;
        let (server, calls) = inference_provider(&f, false).await;
        let workspace = f.root.join("worker-workspace");
        std::fs::create_dir(&workspace).unwrap();
        let mut launch = WorkerLaunch {
            version: 1,
            identity: ExecutionIdentity {
                workspace_id: "workspace-example".into(),
                scan_id: "scan-example".into(),
                agent_id: f.deputy.clone(),
                attempt_id: f.attempt.id.clone(),
                generation: f.attempt.generation,
            },
            leader: f.url.clone(),
            bearer: f.credential.bearer().into(),
            workspace: workspace.clone(),
            protected_root: f.root.clone(),
            recovery: f.root.join("worker-recovery"),
        };
        if unknown {
            let spool = WorkerSpool::open(&launch.recovery, launch.identity.clone()).unwrap();
            spool
                .claim(&WorkRequest::Environment {
                    call: ToolInvocation {
                        id: "uncertain-example".into(),
                        name: "write".into(),
                        arguments: json!({"path":"candidate.txt","content":"Synthetic output"}),
                    },
                })
                .unwrap();
            drop(spool);
            let spool = WorkerSpool::open(&launch.recovery, launch.identity.clone()).unwrap();
            // Delivery of uncertainty must not authorize further research.
            spool.acknowledge("uncertain-example").unwrap();
        }
        let faults: Option<Arc<dyn SpoolFaults>> =
            (!unknown).then(|| Arc::new(FailCheckpoint) as Arc<dyn SpoolFaults>);
        assert!(
            super::worker::run_with_faults(&mut launch, faults)
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(!workspace.join("candidate.txt").exists());
        let progress = f.store.worker_progress(&f.attempt.id).unwrap();
        assert_eq!(progress.revision, 0);
        assert!(progress.answer.is_none());
        let saved = f.store.load("scan-example", true).unwrap();
        assert!(
            saved
                .attempts
                .iter()
                .any(|a| a.id == f.attempt.id && a.state == ExecutionState::Running)
        );
        assert!(
            saved
                .events
                .iter()
                .any(|e| e.payload["event"]["kind"] == "worker_failure")
        );
        server.abort();
        let _ = server.await;
        f.close().await;
    }
}

#[tokio::test]
async fn worker_candidate_progress_and_receipts_roll_back_together_and_replay_after_reopen() {
    let f = Fixture::new().await;
    let checkpoint = f.request(
        "checkpoint-atomic-example",
        WorkerOperation::Checkpoint {
            active_elapsed_ms: 7_500,
            revision: 1,
            context: json!({"synthetic":"checkpoint"}),
        },
    );
    let finish = f.request(
        "finish-atomic-example",
        WorkerOperation::Finish {
            revision: 1,
            answer: "Synthetic candidate answer".into(),
        },
    );
    for request in [&checkpoint, &finish] {
        f.state.db.with_connection(|c| {
            c.execute_batch("CREATE TRIGGER fail_runtime_receipt BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT, 'synthetic checkpoint receipt full'); END;")?;
            Ok(())
        }).unwrap();
        assert_eq!(
            f.client()
                .send(request, Duration::from_secs(5))
                .await
                .unwrap_err()
                .kind,
            TransportFailure::Retryable
        );
        let progress = f.store.worker_progress(&f.attempt.id).unwrap();
        assert_eq!(
            progress.revision,
            if matches!(request.operation, WorkerOperation::Checkpoint { .. }) {
                0
            } else {
                1
            }
        );
        assert_eq!(
            progress.active_elapsed_ms,
            if matches!(request.operation, WorkerOperation::Checkpoint { .. }) {
                0
            } else {
                7_500
            }
        );
        assert!(progress.answer.is_none());
        assert!(
            f.store
                .replay_worker_mutation(f.credential.bearer(), request, unix_ms())
                .unwrap()
                .is_none()
        );
        f.state
            .db
            .with_connection(|c| {
                c.execute_batch("DROP TRIGGER fail_runtime_receipt;")?;
                Ok(())
            })
            .unwrap();
        let accepted = f
            .client()
            .send(request, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(
            f.client()
                .send(request, Duration::from_secs(5))
                .await
                .unwrap(),
            accepted
        );
    }
    let reopened = SwarmStore::open(HostDatabase::open(&f.root.join("host")).unwrap()).unwrap();
    let progress = reopened.worker_progress(&f.attempt.id).unwrap();
    assert_eq!(progress.revision, 1);
    assert_eq!(progress.active_elapsed_ms, 7_500);
    assert_eq!(
        progress.answer.as_deref(),
        Some("Synthetic candidate answer")
    );
    assert_eq!(
        reopened
            .worker_runtime_request(f.credential.bearer(), &finish, unix_ms())
            .unwrap()
            .result["termination_confirmed"],
        false
    );
    assert!(
        reopened
            .load("scan-example", true)
            .unwrap()
            .attempts
            .iter()
            .any(|a| a.id == f.attempt.id && a.state == ExecutionState::Running)
    );
    drop(reopened);
    f.close().await;
}

#[tokio::test]
async fn worker_active_duration_is_monotonic_and_projects_once_after_reopen() {
    let f = Fixture::new().await;
    let checkpoint = |id: &str, revision, elapsed| {
        f.request(id, WorkerOperation::Checkpoint {
        revision, active_elapsed_ms:elapsed, context:json!({"input":[],"phase":{"phase":"done","answer":"Synthetic timed result"}}),
    })
    };
    let first = checkpoint("timed-first-example", 1, 86_400_001);
    f.client()
        .send(&first, Duration::from_secs(5))
        .await
        .unwrap();
    for changed in [
        checkpoint("timed-regressed-example", 2, 86_400_000),
        checkpoint("timed-same-revision-example", 1, 86_400_002),
    ] {
        assert!(
            f.client()
                .send(&changed, Duration::from_secs(5))
                .await
                .is_err()
        );
    }
    let final_checkpoint = checkpoint("timed-final-example", 2, 86_400_250);
    let ack = f
        .client()
        .send(&final_checkpoint, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        f.client()
            .send(&final_checkpoint, Duration::from_secs(5))
            .await
            .unwrap(),
        ack
    );
    f.client()
        .send(
            &f.request(
                "timed-finish-example",
                WorkerOperation::Finish {
                    revision: 2,
                    answer: "Synthetic timed result".into(),
                },
            ),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    let reopened = SwarmStore::open(HostDatabase::open(&f.root.join("host")).unwrap()).unwrap();
    assert_eq!(
        reopened
            .worker_progress(&f.attempt.id)
            .unwrap()
            .active_elapsed_ms,
        86_400_250
    );
    for _ in 0..2 {
        reopened
            .reconcile_worker(&f.attempt.id, true, WorkerDisposition::Completed, unix_ms())
            .unwrap();
    }
    assert_eq!(
        f.state.db.session_total_duration_ms(&f.deputy).unwrap(),
        86_400_250
    );
    assert!(
        f.client()
            .send(&final_checkpoint, Duration::from_secs(5))
            .await
            .is_err()
    );
    drop(reopened);
    f.close().await;
}

#[tokio::test]
async fn worker_rejects_recovery_outside_private_root_before_creating_it() {
    let f = Fixture::new().await;
    let protected = f.root.join("private");
    let workspace = protected.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let recovery = f.root.join("rejected-recovery");
    let launch = WorkerLaunch {
        version: 1,
        identity: beale_contracts::swarm_worker::ExecutionIdentity {
            workspace_id: "workspace-example".into(),
            scan_id: "scan-example".into(),
            agent_id: f.deputy.clone(),
            attempt_id: f.attempt.id.clone(),
            generation: f.attempt.generation,
        },
        leader: f.url.clone(),
        bearer: f.credential.bearer().into(),
        workspace,
        protected_root: protected,
        recovery: recovery.clone(),
    };
    assert!(run_assigned(launch).await.is_err());
    assert!(!recovery.exists());
    f.close().await;
}
fn acknowledged(reply: WorkerReply) -> Value {
    let WorkerReply::Acknowledged { acknowledgement } = reply else {
        panic!("expected durable acknowledgement")
    };
    acknowledgement.result
}

#[cfg(feature = "surfaces")]
#[tokio::test]
async fn worker_surface_mutations_recover_file_commit_without_repeating_intervening_edits() {
    let workspace_id = "11111111-1111-4111-8111-111111111111";
    let f = Fixture::with_workspace(
        true,
        beale_contracts::swarm_worker::WorkerShell::native(),
        false,
        workspace_id,
    )
    .await;
    let workspace = f.root.join("workspace");
    std::fs::create_dir_all(workspace.join(".beale")).unwrap();
    std::fs::write(workspace.join("workspace.json"), json!({"schemaVersion":7,"workspaceId":workspace_id,"name":"Example","directories":[],"surfaces":[],"rules":[],"foundationRevision":1,"surfacesRevision":1,"rulesRevision":1,"nextSurfaceNumber":1}).to_string()).unwrap();
    let create = f.request("surface-create-example", WorkerOperation::Canonical { tool:"surface_save".into(), arguments:json!({"operation":"create","definition":{"type_id":"repository","name":"Example repository","locator":{"kind":"freeform","value":"synthetic repository"},"scope":"in_scope"}}) });
    let mut invalid_artifact = create.clone();
    invalid_artifact.operation_id = "surface-unaccepted-evidence".into();
    let WorkerOperation::Canonical { arguments, .. } = &mut invalid_artifact.operation else {
        unreachable!()
    };
    arguments["definition"]["evidence_refs"] =
        json!([{"kind":"artifact","reference":format!("artifact-{}", "f".repeat(64))}]);
    let rejected = acknowledged(
        f.client()
            .send(&invalid_artifact, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    assert_eq!(rejected["is_error"], true);
    assert_eq!(rejected["error"]["code"], "rejected");
    assert!(
        f.state
            .workspaces
            .surfaces_for_host(workspace_id)
            .unwrap()
            .surfaces
            .is_empty()
    );
    let fail_receipts = || {
        f.state.db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_surface_receipt BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT, 'synthetic Surface receipt full'); END;")?; Ok(()) }).unwrap()
    };
    let restore_receipts = || {
        f.state
            .db
            .with_connection(|c| {
                c.execute_batch("DROP TRIGGER fail_surface_receipt;")?;
                Ok(())
            })
            .unwrap()
    };
    fail_receipts();
    assert!(
        f.client()
            .send(&create, Duration::from_secs(5))
            .await
            .is_err()
    );
    let saved = f.state.workspaces.surfaces_for_host(workspace_id).unwrap();
    assert_eq!(saved.revision, 2);
    assert_eq!(saved.surfaces.len(), 1);
    assert!(
        f.store
            .replay_worker_mutation(f.credential.bearer(), &create, unix_ms())
            .unwrap()
            .is_none()
    );
    beale_surfaces::execute(&f.state.workspaces, workspace_id, beale_surfaces::SAVE, &json!({"operation":"revise","id":"S1","expected_revision":2,"definition":{"name":"Intervening example edit"}}), true).unwrap();
    let mut changed = create.clone();
    let WorkerOperation::Canonical { arguments, .. } = &mut changed.operation else {
        unreachable!()
    };
    arguments["definition"]["name"] = json!("Changed retry");
    assert!(
        f.client()
            .send(&changed, Duration::from_secs(5))
            .await
            .is_err()
    );
    restore_receipts();
    f.lose_response.store(true, Ordering::SeqCst);
    assert!(
        f.client()
            .send(&create, Duration::from_secs(5))
            .await
            .is_err()
    );
    let accepted = acknowledged(
        f.client()
            .send(&create, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    assert_eq!(accepted["workspace_surfaces_revision"], 2);
    assert_eq!(accepted["definition"]["name"], "Example repository");
    let current = f.state.workspaces.surfaces_for_host(workspace_id).unwrap();
    assert_eq!(current.revision, 3);
    assert_eq!(current.surfaces.len(), 1);
    assert_eq!(
        current.surfaces[0].definition.as_ref().unwrap().name,
        "Intervening example edit"
    );
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(workspace.join("workspace.json")).unwrap()).unwrap();
    assert!(manifest.get("surfaceReceipts").is_none());
    let scenario = json!({"title":"Synthetic scenario","attacker_position":"Example input","preconditions":["Synthetic setup"],"suspected_failure":"Example boundary","invariant_at_risk":"Example invariant","potential_impact":"Synthetic effect","validation_question":"Check synthetic behavior"});
    let threat = f.request(
        "threat-create-example",
        WorkerOperation::Canonical {
            tool: "surface_threat_model_edit".into(),
            arguments: json!({"surface_id":"S1","scenario":scenario}),
        },
    );
    fail_receipts();
    assert!(
        f.client()
            .send(&threat, Duration::from_secs(5))
            .await
            .is_err()
    );
    let model = f
        .state
        .workspaces
        .threat_model_for_host(workspace_id, "S1")
        .unwrap();
    assert_eq!(model.revision, 2);
    assert_eq!(model.scenarios.len(), 1);
    beale_surfaces::execute(
        &f.state.workspaces,
        workspace_id,
        beale_surfaces::THREAT_MODEL_EDIT,
        &json!({"surface_id":"S1","remove_id":"T1"}),
        true,
    )
    .unwrap();
    restore_receipts();
    let accepted = acknowledged(
        f.client()
            .send(&threat, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    assert_eq!(accepted["revision"], 2);
    assert_eq!(accepted["changed_id"], "T1");
    let model = f
        .state
        .workspaces
        .threat_model_for_host(workspace_id, "S1")
        .unwrap();
    assert_eq!(model.revision, 3);
    assert!(model.scenarios.is_empty());
    let saved: Value = serde_json::from_slice(
        &std::fs::read(workspace.join(".beale/surface-threat-models/S1.json")).unwrap(),
    )
    .unwrap();
    assert!(saved.get("surface_receipts").is_none() && saved.get("surfaceReceipts").is_none());
    let pending = f.request("surface-pending-cancel", WorkerOperation::Canonical { tool:"surface_save".into(), arguments:json!({"operation":"revise","id":"S1","expected_revision":3,"definition":{"name":"Retained unacknowledged candidate"}}) });
    fail_receipts();
    assert!(
        f.client()
            .send(&pending, Duration::from_secs(5))
            .await
            .is_err()
    );
    restore_receipts();
    f.store.local_stop("scan-example", &f.deputy).unwrap();
    assert!(
        f.client()
            .send(&create, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert!(
        f.client()
            .send(&threat, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert!(
        f.client()
            .send(&pending, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert!(
        f.store
            .replay_worker_mutation(f.credential.bearer(), &pending, unix_ms())
            .is_err()
    );
    let retained: Value =
        serde_json::from_slice(&std::fs::read(workspace.join("workspace.json")).unwrap()).unwrap();
    assert_eq!(retained["surfaceReceipts"].as_array().unwrap().len(), 1);
    assert_eq!(
        f.state
            .workspaces
            .surfaces_for_host(workspace_id)
            .unwrap()
            .revision,
        4
    );
    f.close().await;
}

#[cfg(feature = "surfaces")]
#[tokio::test]
async fn worker_surface_reads_replay_accepted_records_with_their_matching_revision() {
    let f = Fixture::with_workspace(
        true,
        beale_contracts::swarm_worker::WorkerShell::native(),
        false,
        "11111111-1111-4111-8111-111111111111",
    )
    .await;
    let workspace = f.root.join("workspace");
    std::fs::create_dir_all(workspace.join(".beale")).unwrap();
    std::fs::write(workspace.join("workspace.json"), json!({"schemaVersion":7,"workspaceId":"11111111-1111-4111-8111-111111111111","name":"Example","directories":[],"surfaces":[],"rules":[],"foundationRevision":1,"surfacesRevision":1,"rulesRevision":1,"nextSurfaceNumber":1}).to_string()).unwrap();
    beale_surfaces::execute(&f.state.workspaces, "11111111-1111-4111-8111-111111111111", beale_surfaces::SAVE, &json!({"operation":"create","definition":{"type_id":"repository","name":"Example repository","locator":{"kind":"freeform","value":"synthetic repository"},"scope":"in_scope"}}), true).unwrap();
    let read = f.request(
        "surface-read-example",
        WorkerOperation::Canonical {
            tool: "surface_read".into(),
            arguments: json!({"surface_id":"S1","section":"record"}),
        },
    );
    f.lose_response.store(true, Ordering::SeqCst);
    assert!(
        f.client()
            .send(&read, Duration::from_secs(5))
            .await
            .is_err()
    );
    let result = acknowledged(
        f.client()
            .send(&read, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    assert_eq!(
        result["surface"]["definition"]["name"],
        "Example repository"
    );
    assert_eq!(result["workspace_surfaces_revision"], 2);
    let captured = f
        .state
        .workspaces
        .surface_with_revision_for_host("11111111-1111-4111-8111-111111111111", "S1")
        .unwrap()
        .unwrap();
    beale_surfaces::execute(&f.state.workspaces, "11111111-1111-4111-8111-111111111111", beale_surfaces::SAVE, &json!({"operation":"revise","id":"S1","expected_revision":2,"definition":{"name":"Updated example repository"}}), true).unwrap();
    assert_eq!(captured.0.definition.unwrap().name, "Example repository");
    assert_eq!(captured.2, 2);
    assert_eq!(
        acknowledged(
            f.client()
                .send(&read, Duration::from_secs(5))
                .await
                .unwrap()
        ),
        result
    );
    let mut fresh = read.clone();
    fresh.operation_id = "surface-read-fresh".into();
    let updated = acknowledged(
        f.client()
            .send(&fresh, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    assert_eq!(
        updated["surface"]["definition"]["name"],
        "Updated example repository"
    );
    assert_eq!(updated["workspace_surfaces_revision"], 3);
    let query = f.request(
        "surface-query-example",
        WorkerOperation::Canonical {
            tool: "surface_query".into(),
            arguments: json!({"workspace_id":"foreign-example"}),
        },
    );
    let result = acknowledged(
        f.client()
            .send(&query, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    assert!(result.to_string().contains("Updated example repository"));
    let foreign = f.request("surface-read-foreign", WorkerOperation::Canonical { tool:"surface_read".into(), arguments:json!({"surface_id":"S1","section":"record","workspace_id":"foreign-example"}) });
    let rejected = acknowledged(
        f.client()
            .send(&foreign, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    assert_eq!(rejected["is_error"], true);
    assert_eq!(rejected["error"]["code"], "invalid_arguments");
    assert!(rejected.get("surface").is_none());
    let threat = f.request(
        "surface-threat-example",
        WorkerOperation::Canonical {
            tool: "surface_read".into(),
            arguments: json!({"surface_id":"S1","section":"threat_model"}),
        },
    );
    assert!(
        f.client()
            .send(&threat, Duration::from_secs(5))
            .await
            .is_ok()
    );
    let workspaces = f.state.workspaces.clone();
    let workspace_id = f.workspace_id.clone();
    let writer = std::thread::spawn(move || {
        for revision in 4..36 {
            beale_surfaces::execute(&workspaces, &workspace_id, beale_surfaces::SAVE, &json!({"operation":"revise","id":"S1","expected_revision":revision-1,"definition":{"name":format!("Version {revision}")}}), true).unwrap();
        }
    });
    for index in 0..32 {
        let mut concurrent = read.clone();
        concurrent.operation_id = format!("surface-concurrent-{index}");
        let record = acknowledged(
            f.client()
                .send(&concurrent, Duration::from_secs(5))
                .await
                .unwrap(),
        );
        let name = record["surface"]["definition"]["name"].as_str().unwrap();
        let revision = record["workspace_surfaces_revision"].as_u64().unwrap();
        if let Some(version) = name.strip_prefix("Version ") {
            assert_eq!(version.parse::<u64>().unwrap(), revision);
        } else {
            assert_eq!(revision, 3);
        }
    }
    writer.join().unwrap();
    f.store.local_stop("scan-example", &f.deputy).unwrap();
    assert!(
        f.client()
            .send(&read, Duration::from_secs(5))
            .await
            .is_err()
    );
    f.close().await;
}

#[cfg(feature = "agent-feedback")]
#[tokio::test]
async fn worker_feedback_retries_file_effect_after_receipt_failure_without_duplication() {
    let f = Fixture::new().await;
    let request = f.request("feedback-example", WorkerOperation::Canonical { tool: "agent_feedback".into(), arguments:json!({"kind":"limitation","observation":format!("Synthetic display delay {}", f.state.token)}) });
    f.state.db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_feedback_receipt BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT, 'synthetic feedback receipt full'); END;")?; Ok(()) }).unwrap();
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    let path = f
        .state
        .workspaces
        .root()
        .join(beale_agent_feedback::FILE_NAME);
    let original = std::fs::read(&path).unwrap();
    assert_eq!(std::str::from_utf8(&original).unwrap().lines().count(), 1);
    assert!(
        !std::str::from_utf8(&original)
            .unwrap()
            .contains(&f.state.token)
    );
    assert!(
        f.store
            .replay_worker_mutation(f.credential.bearer(), &request, unix_ms())
            .unwrap()
            .is_none()
    );
    f.state
        .db
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER fail_feedback_receipt;")?;
            Ok(())
        })
        .unwrap();
    f.lose_response.store(true, Ordering::SeqCst);
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    let result = acknowledged(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    assert_eq!(result["is_error"], false);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let record: Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(record["workspace_id"], "workspace-example");
    assert_eq!(record["agent_id"], f.deputy);
    assert_eq!(
        record["session_id"],
        f.store
            .worker_progress(&f.attempt.id)
            .unwrap()
            .turn
            .session_id
    );
    let mut changed = request.clone();
    let WorkerOperation::Canonical { arguments, .. } = &mut changed.operation else {
        unreachable!()
    };
    arguments["observation"] = json!("Changed synthetic observation");
    assert!(
        f.client()
            .send(&changed, Duration::from_secs(5))
            .await
            .is_err()
    );
    f.store.local_stop("scan-example", &f.deputy).unwrap();
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    f.close().await;
}

#[cfg(feature = "memories")]
#[tokio::test]
async fn worker_memory_reads_are_host_owned_scoped_and_replay_the_accepted_snapshot() {
    let f = Fixture::new().await;
    let memory = beale_memories::MemoryStore::open(f.state.workspaces.root()).unwrap();
    for (id, workspace, name) in [
        (1, "workspace-example", "Example component"),
        (2, "workspace-other", "Other synthetic component"),
    ] {
        memory
            .commit(
                &beale_memories::MemoryTurnJob {
                    id,
                    workspace_id: workspace.into(),
                    session_id: "session-example".into(),
                    turn_number: 1,
                    agent_id: "agent-example".into(),
                },
                "example-model",
                beale_memories::Extraction {
                    entities: vec![beale_memories::EntityProposal {
                        name: name.into(),
                        kind: "component".into(),
                        canonical_ref: String::new(),
                    }],
                    assertions: vec![],
                },
            )
            .unwrap();
    }
    let query = f.request(
        "memory-query-example",
        WorkerOperation::Canonical {
            tool: "memory_query".into(),
            arguments: json!({"workspace_id":"workspace-other"}),
        },
    );
    f.lose_response.store(true, Ordering::SeqCst);
    assert!(
        f.client()
            .send(&query, Duration::from_secs(5))
            .await
            .is_err()
    );
    let snapshot = acknowledged(
        f.client()
            .send(&query, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    assert_eq!(snapshot["items"].as_array().unwrap().len(), 1);
    assert!(snapshot.to_string().contains("Example component"));
    assert!(!snapshot.to_string().contains("Other synthetic component"));
    let id = snapshot["items"][0]["id"].as_str().unwrap();
    for tool in ["memory_read", "memory_graph"] {
        let result = acknowledged(
            f.client()
                .send(
                    &f.request(
                        tool,
                        WorkerOperation::Canonical {
                            tool: tool.into(),
                            arguments: json!({"id":id}),
                        },
                    ),
                    Duration::from_secs(5),
                )
                .await
                .unwrap(),
        );
        assert!(result.to_string().contains("Example component"));
    }
    let foreign = memory
        .query(beale_memories::MemoryQuery {
            workspace_id: "workspace-other",
            text: "",
            kind: "",
            status: "",
            as_of_ms: None,
            offset: 0,
            limit: 20,
        })
        .unwrap();
    let read = f.request(
        "foreign-memory-read",
        WorkerOperation::Canonical {
            tool: "memory_read".into(),
            arguments: json!({"id":foreign["items"][0]["id"],"workspace_id":"workspace-other"}),
        },
    );
    let result = f.client().send(&read, Duration::from_secs(5)).await;
    assert!(
        result.is_err()
            || !serde_json::to_string(&result.unwrap())
                .unwrap()
                .contains("Other synthetic component")
    );
    // The deputy has no committed own turns; the leader already has one.
    let sessions = f.request(
        "memory-session-query",
        WorkerOperation::Canonical {
            tool: "memory_session_query".into(),
            arguments: json!({"session_id":f.leader}),
        },
    );
    assert_eq!(
        acknowledged(
            f.client()
                .send(&sessions, Duration::from_secs(5))
                .await
                .unwrap()
        )["items"],
        json!([])
    );
    memory.clear_workspace("workspace-example").unwrap();
    assert_eq!(
        acknowledged(
            f.client()
                .send(&query, Duration::from_secs(5))
                .await
                .unwrap()
        ),
        snapshot
    );
    let mut fresh = query;
    fresh.operation_id = "memory-query-fresh".into();
    assert_eq!(
        acknowledged(
            f.client()
                .send(&fresh, Duration::from_secs(5))
                .await
                .unwrap()
        )["items"],
        json!([])
    );
    f.store.local_stop("scan-example", &f.deputy).unwrap();
    assert!(
        f.client()
            .send(&fresh, Duration::from_secs(5))
            .await
            .is_err()
    );
    drop(memory);
    f.close().await;
}

#[cfg(feature = "runbooks")]
#[tokio::test]
async fn worker_runbook_records_and_receipts_commit_atomically_under_saved_policy() {
    use beale_runbooks::RunbookStore;
    let f = Fixture::new().await;
    f.state
        .db
        .set_feature_enabled(beale_runbooks::FEATURE_ID, false)
        .unwrap();
    let create = f.request("create-runbook-example", WorkerOperation::Canonical { tool: "runbook_create".into(), arguments: json!({"title":"Example procedure","purpose":"Synthetic workflow","cells":[{"kind":"code","id":"","language":"bash","source":"printf 'example'"}]}) });
    f.state.db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_runbook_receipt BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT, 'synthetic runbook receipt full'); END;")?; Ok(()) }).unwrap();
    assert!(
        f.client()
            .send(&create, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert!(
        f.state
            .db
            .runbook_get("workspace-example", "R1")
            .unwrap()
            .is_none()
    );
    f.state
        .db
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER fail_runbook_receipt;")?;
            Ok(())
        })
        .unwrap();
    f.lose_response.store(true, Ordering::SeqCst);
    assert!(
        f.client()
            .send(&create, Duration::from_secs(5))
            .await
            .is_err()
    );
    let accepted = acknowledged(
        f.client()
            .send(&create, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    assert_eq!(accepted["runbook"]["id"], "R1");
    assert_eq!(accepted["runbook"]["revision"], 1);
    assert!(
        f.state
            .db
            .runbook_get("workspace-example", "R2")
            .unwrap()
            .is_none()
    );
    let edit = f.request("edit-runbook-example", WorkerOperation::Canonical { tool: "runbook_edit".into(), arguments: json!({"id":"R1","expected_revision":1,"reason":"Synthetic revision","purpose":"Updated synthetic workflow"}) });
    f.state.db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_runbook_edit_receipt BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT, 'synthetic runbook edit receipt full'); END;")?; Ok(()) }).unwrap();
    assert!(
        f.client()
            .send(&edit, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert_eq!(
        f.state
            .db
            .runbook_get("workspace-example", "R1")
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    f.state
        .db
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER fail_runbook_edit_receipt;")?;
            Ok(())
        })
        .unwrap();
    let revised = f
        .client()
        .send(&edit, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        f.client()
            .send(&edit, Duration::from_secs(5))
            .await
            .unwrap(),
        revised
    );
    assert_eq!(
        f.state
            .db
            .runbook_get("workspace-example", "R1")
            .unwrap()
            .unwrap()
            .revision,
        2
    );
    let read = f.request(
        "read-runbook-example",
        WorkerOperation::Canonical {
            tool: "runbook_get".into(),
            arguments: json!({"id":"R1"}),
        },
    );
    assert_eq!(
        acknowledged(
            f.client()
                .send(&read, Duration::from_secs(5))
                .await
                .unwrap()
        )["runbook"]["purpose"],
        "Updated synthetic workflow"
    );
    let foreign = f.request(
        "foreign-runbook-example",
        WorkerOperation::Canonical {
            tool: "runbook_get".into(),
            arguments: json!({"id":"R1","workspace_id":"foreign-example"}),
        },
    );
    // Caller-supplied ownership cannot select another workspace.
    assert_eq!(
        acknowledged(
            f.client()
                .send(&foreign, Duration::from_secs(5))
                .await
                .unwrap()
        )["runbook"]["workspace_id"],
        "workspace-example"
    );
    let turn = f.store.worker_progress(&f.attempt.id).unwrap().turn;
    let run = f
        .state
        .db
        .runbook_run_start("workspace-example", "foreign-session", "R1", 2)
        .unwrap();
    let status = f.request(
        "foreign-run-status",
        WorkerOperation::Canonical {
            tool: "runbook_status".into(),
            arguments: json!({"run_id":run.id}),
        },
    );
    let denied = acknowledged(
        f.client()
            .send(&status, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    assert_eq!(denied["is_error"], true);
    assert_eq!(denied["error"]["code"], "not_found");
    assert_eq!(denied["output"], "runbook execution not found");
    assert_eq!(
        acknowledged(
            f.client()
                .send(&status, Duration::from_secs(5))
                .await
                .unwrap()
        ),
        denied
    );
    let own_run = f
        .state
        .db
        .runbook_run_start("workspace-example", &turn.session_id, "R1", 2)
        .unwrap();
    let status = f.request(
        "own-run-status",
        WorkerOperation::Canonical {
            tool: "runbook_status".into(),
            arguments: json!({"run_id":own_run.id}),
        },
    );
    assert_eq!(
        acknowledged(
            f.client()
                .send(&status, Duration::from_secs(5))
                .await
                .unwrap()
        )["status"],
        "running"
    );
    let secret = f.request("redact-runbook-example", WorkerOperation::Canonical { tool:"runbook_edit".into(), arguments:json!({"id":"R1","expected_revision":2,"reason":"Synthetic redaction check","purpose":format!("Synthetic {}",f.state.token)}) });
    let result = f
        .client()
        .send(&secret, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains(&f.state.token)
    );
    f.store.local_stop("scan-example", &f.deputy).unwrap();
    assert!(
        f.client()
            .send(&create, Duration::from_secs(5))
            .await
            .is_err()
    );
    f.close().await;
}

#[cfg(feature = "findings")]
#[tokio::test]
async fn assigned_worker_publishes_finding_attachments_and_materializes_verified_handoffs() {
    use beale_findings::FindingStore;
    let f = Fixture::new().await;
    let workspace = f.root.join("worker-workspace");
    std::fs::create_dir(&workspace).unwrap();
    let original = vec![b'e'; 192 * 1024 + 17];
    std::fs::write(workspace.join("proof.txt"), &original).unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = calls.clone();
    let db = f.state.db.clone();
    let source = workspace.join("proof.txt");
    let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(_): axum::Json<Value>| {
        let calls = captured.clone(); let db = db.clone(); let source = source.clone();
        async move {
            let delta = match calls.fetch_add(1, Ordering::SeqCst) {
                0 => json!({"tool_calls":[{"index":0,"id":"finding-example","type":"function","function":{"name":"findings_create","arguments":json!({"title":"Synthetic candidate","summary":"Synthetic evidence","hypothesis":"Example boundary","root_cause":{"summary":"Candidate explanation","evidence_refs":[{"kind":"artifact","reference":"file:proof.txt"}]}}).to_string()}}]}),
                1 => {
                    let finding = serde_json::to_value(db.finding_get("workspace-example", "F1").unwrap().unwrap()).unwrap();
                    let artifact = finding["root_cause"]["evidence_refs"][0]["reference"].as_str().unwrap();
                    assert!(artifact.starts_with("artifact-"));
                    std::fs::write(source, "Changed after durable publication").unwrap();
                    json!({"tool_calls":[{"index":0,"id":"materialize-example","type":"function","function":{"name":"swarm_materialize_artifact","arguments":json!({"artifact":artifact,"destination":"received/proof.txt"}).to_string()}}]})
                }
                2 => json!({"content":"Synthetic artifact handoff complete"}),
                _ => panic!("unexpected repeated model turn"),
            };
            let chunk = json!({"choices":[{"delta":delta}]});
            ([("content-type","text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
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
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let launch = || WorkerLaunch {
        version: 1,
        identity: f.store.worker_bootstrap(&f.attempt.id).unwrap().identity,
        leader: f.url.clone(),
        bearer: f.credential.bearer().into(),
        workspace: workspace.clone(),
        protected_root: f.root.clone(),
        recovery: f.root.join("worker-recovery"),
    };
    tokio::time::timeout(Duration::from_secs(15), run_assigned(launch()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        std::fs::read(workspace.join("received/proof.txt")).unwrap(),
        original
    );
    assert!(
        f.state
            .db
            .finding_get("workspace-example", "F2")
            .unwrap()
            .is_none()
    );
    let finding = serde_json::to_value(
        f.state
            .db
            .finding_get("workspace-example", "F1")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let id = finding["root_cause"]["evidence_refs"][0]["reference"]
        .as_str()
        .unwrap();
    let artifact = crate::research_files::store(&f.state)
        .unwrap()
        .artifact("workspace-example", id)
        .unwrap();
    assert_eq!(artifact.attempt, f.attempt.id);
    assert_eq!(artifact.producer, f.deputy);
    assert_eq!(artifact.size, original.len() as u64);
    std::fs::write(workspace.join("received/proof.txt"), "Operator edit").unwrap();
    run_assigned(launch()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.join("received/proof.txt")).unwrap(),
        "Operator edit"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn assigned_worker_returns_durable_tool_errors_and_continues_without_effects() {
    assigned_worker_feedback_recovery(2, 1).await;
}

#[tokio::test]
async fn assigned_worker_review_denials_survive_checkpoint_failure_without_execution() {
    assigned_worker_feedback_recovery(9, 8).await;
    #[cfg(feature = "findings")]
    assigned_worker_feedback_recovery(10, 9).await;
}

async fn assigned_worker_feedback_recovery(checkpoint_fault: usize, retained_outcomes: usize) {
    let f = Fixture::with_shell(
        true,
        beale_contracts::swarm_worker::WorkerShell::native(),
        true,
    )
    .await;
    for rule in beale_safety::rules(&f.state.db).unwrap() {
        f.state
            .db
            .save_permission_rule(&rule.id, &rule.title, &rule.instruction, true)
            .unwrap();
    }
    let expected_outcomes = 9 + usize::from(cfg!(feature = "findings"));
    let workspace = f.root.join("swarm-files/environments/worker-example");
    std::fs::create_dir_all(&workspace).unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = calls.clone();
    let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let calls = captured.clone();
        async move {
            if let Some(input) = body["messages"][1]["content"].as_str().and_then(|text| serde_json::from_str::<Value>(text).ok()).filter(|v| v.get("request").is_some()) {
                let denied = input["request"]["tool"] == beale_tools::COMMAND_TOOL_NAME || input["request"]["tool"] == "findings_create";
                let content = json!({"decision":if denied { "denied" } else { "approved" },"reason":"Synthetic policy decision"}).to_string();
                let chunk = json!({"choices":[{"delta":{"content":content}}]});
                return ([("content-type","text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"));
            }
            let delta = match calls.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    let cases = [
                        ("read", "{broken".to_owned()),
                        ("write", "[]".into()),
                        ("unavailable_example", "{}".into()),
                        ("write", json!({"path":"must-not-exist.txt","content":false}).to_string()),
                        (beale_tools::COMMAND_TOOL_NAME, json!({"command":"synthetic-command-must-not-run","timeout_seconds":0}).to_string()),
                        ("swarm_materialize_artifact", json!({"artifact":"artifact-example","destination":"../escape"}).to_string()),
                        ("read", json!({"path":"missing-example.txt"}).to_string()),
                        (beale_tools::COMMAND_TOOL_NAME, json!({"command":"synthetic-denied-command-must-not-run"}).to_string()),
                        #[cfg(feature = "findings")]
                        ("findings_create", json!({"title":"Synthetic denied finding","summary":"Must not persist","hypothesis":"Synthetic invariant"}).to_string()),
                        ("write", json!({"path":"result.txt","content":"Synthetic recovery succeeded"}).to_string()),
                    ];
                    let tools: Vec<_> = cases.into_iter().enumerate().map(|(index,(name,arguments))| json!({"index":index,"id":format!("feedback-example-{index}"),"type":"function","function":{"name":name,"arguments":arguments}})).collect();
                    json!({"tool_calls":tools})
                }
                1 => {
                    let outputs: Vec<_> = body["messages"].as_array().unwrap().iter().filter(|m| m["role"] == "tool").collect();
                    assert_eq!(outputs.len(), expected_outcomes);
                    assert!(outputs[0]["content"].as_str().unwrap().contains("valid JSON object"));
                    assert!(outputs[2]["content"].as_str().unwrap().contains("unavailable"));
                    assert!(outputs[7]["content"].as_str().unwrap().contains("did not approve"));
                    #[cfg(feature = "findings")]
                    assert!(outputs[8]["content"].as_str().unwrap().contains("did not approve"));
                    assert!(outputs.last().unwrap()["content"].as_str().unwrap().contains("Wrote"));
                    json!({"content":"Synthetic tool feedback handled"})
                }
                _ => panic!("unexpected repeated model turn"),
            };
            let chunk = json!({"choices":[{"delta":delta}]});
            ([("content-type","text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
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
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let launch = || WorkerLaunch {
        version: 1,
        identity: f.store.worker_bootstrap(&f.attempt.id).unwrap().identity,
        leader: f.url.clone(),
        bearer: f.credential.bearer().into(),
        workspace: workspace.clone(),
        protected_root: f.root.clone(),
        recovery: f.root.join("worker-recovery"),
    };
    struct FailFeedbackCheckpoint(std::sync::atomic::AtomicUsize, usize);
    impl beale_storage::worker_spool::SpoolFaults for FailFeedbackCheckpoint {
        fn before_commit(
            &self,
            write: beale_storage::worker_spool::SpoolWrite,
        ) -> anyhow::Result<()> {
            if write == beale_storage::worker_spool::SpoolWrite::Checkpoint {
                anyhow::ensure!(
                    self.0.fetch_add(1, Ordering::SeqCst) != self.1,
                    "synthetic feedback checkpoint full"
                );
            }
            Ok(())
        }
    }
    assert!(
        super::worker::run_with_faults(
            &mut launch(),
            Some(Arc::new(FailFeedbackCheckpoint(
                std::sync::atomic::AtomicUsize::new(0),
                checkpoint_fault
            )))
        )
        .await
        .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .events
            .iter()
            .filter(|e| e.payload["event"]["kind"] == "tool_outcome")
            .count(),
        retained_outcomes
    );
    assert!(!workspace.join("result.txt").exists());
    run_assigned(launch()).await.unwrap();
    assert!(!workspace.join("must-not-exist.txt").exists());
    assert_eq!(
        std::fs::read_to_string(workspace.join("result.txt")).unwrap(),
        "Synthetic recovery succeeded"
    );
    let scan = f.store.load("scan-example", true).unwrap();
    let outcomes: Vec<_> = scan
        .events
        .iter()
        .filter(|e| e.payload["event"]["kind"] == "tool_outcome")
        .collect();
    assert_eq!(outcomes.len(), expected_outcomes);
    assert_eq!(
        outcomes
            .iter()
            .filter(|e| e.payload["event"]["result"]["is_error"] == true)
            .count(),
        expected_outcomes - 1
    );
    #[cfg(feature = "findings")]
    {
        use beale_findings::FindingStore;
        assert!(
            f.state
                .db
                .finding_get("workspace-example", "F1")
                .unwrap()
                .is_none()
        );
    }
    run_assigned(launch()).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .events
            .iter()
            .filter(|e| e.payload["event"]["kind"] == "tool_outcome")
            .count(),
        expected_outcomes
    );
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn assigned_worker_retains_large_output_before_transfer_and_retries_without_rereading() {
    let f = Fixture::new().await;
    let workspace = f.root.join("swarm-files/environments/worker-example");
    std::fs::create_dir_all(&workspace).unwrap();
    let original = "Synthetic output \u{e9} ".repeat(20_000);
    std::fs::write(workspace.join("large.txt"), &original).unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = calls.clone();
    let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let calls = captured.clone();
        async move {
            let delta = match calls.fetch_add(1, Ordering::SeqCst) {
                0 => json!({"tool_calls":[{"index":0,"id":"large-output-example","type":"function","function":{"name":"read","arguments":json!({"path":"large.txt"}).to_string()}}]}),
                1 => {
                    let output = body["messages"].as_array().unwrap().iter().find(|m| m["role"] == "tool").unwrap()["content"].as_str().unwrap();
                    assert!(output.len() < 40_000);
                    assert!(output.contains("Accepted full-output artifact: artifact-"));
                    json!({"content":"Synthetic output retained"})
                }
                _ => panic!("unexpected repeated model turn"),
            };
            let chunk = json!({"choices":[{"delta":delta}]});
            ([("content-type","text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
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
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let launch = || WorkerLaunch {
        version: 1,
        identity: f.store.worker_bootstrap(&f.attempt.id).unwrap().identity,
        leader: f.url.clone(),
        bearer: f.credential.bearer().into(),
        workspace: workspace.clone(),
        protected_root: f.root.clone(),
        recovery: f.root.join("worker-recovery"),
    };
    let _files = crate::research_files::store(&f.state).unwrap();
    f.state.db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_large_output BEFORE UPDATE OF accepted ON swarm_file_transfers WHEN NEW.accepted=1 BEGIN SELECT RAISE(ABORT, 'synthetic output acceptance full'); END;")?; Ok(()) }).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(15), run_assigned(launch()))
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    std::fs::write(
        workspace.join("large.txt"),
        "Changed after completed local read",
    )
    .unwrap();
    f.state
        .db
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER fail_large_output;")?;
            Ok(())
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), run_assigned(launch()))
        .await
        .unwrap()
        .unwrap();
    let scan = f.store.load("scan-example", true).unwrap();
    let outcomes: Vec<_> = scan
        .events
        .iter()
        .filter(|e| e.payload["event"]["kind"] == "tool_outcome")
        .collect();
    assert_eq!(outcomes.len(), 1);
    let result = &outcomes[0].payload["event"]["result"];
    let files = crate::research_files::store(&f.state).unwrap();
    let artifact = files
        .artifact("workspace-example", result["artifact"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        artifact.sha256,
        beale_storage::swarm_files::hash(format!("1\t{original}\n").as_bytes())
    );
    assert_eq!(artifact.size, (original.len() + 3) as u64);
    assert_eq!(
        std::fs::read_to_string(workspace.join(result["candidate_output"].as_str().unwrap()))
            .unwrap(),
        format!("1\t{original}\n")
    );
    run_assigned(launch()).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    drop(files);
    drop(_files);
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[cfg(feature = "webfetch")]
#[tokio::test]
async fn assigned_webfetch_replays_completed_fetch_after_checkpoint_failure() {
    let f = Fixture::new().await;
    let workspace = f.root.join("worker-workspace");
    std::fs::create_dir(&workspace).unwrap();
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fetched = hits.clone();
    let bearer = f.credential.bearer().to_owned();
    let page = axum::Router::new().route("/example", axum::routing::get(move |headers: axum::http::HeaderMap| {
        let fetched = fetched.clone(); let bearer = bearer.clone();
        async move {
            assert!(!headers.contains_key("authorization"));
            fetched.fetch_add(1, Ordering::SeqCst);
            ([("content-type", "text/html")], format!("<p>Synthetic worker page {bearer}</p><script>hidden_example_script()</script>"))
        }
    }));
    let page_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let page_url = format!("http://{}/example", page_listener.local_addr().unwrap());
    let page_server = tokio::spawn(async move {
        axum::serve(page_listener, page).await.unwrap();
    });
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = calls.clone();
    let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let calls = captured.clone(); let page_url = page_url.clone();
        async move {
            let delta = match calls.fetch_add(1, Ordering::SeqCst) {
                0 => json!({"tool_calls":[
                    {"index":0,"id":"invalid-fetch-example","type":"function","function":{"name":"webfetch","arguments":json!({"url":page_url,"query":"Example query"}).to_string()}},
                    {"index":1,"id":"http-error-example","type":"function","function":{"name":"webfetch","arguments":json!({"url":format!("{page_url}/missing")}).to_string()}},
                    {"index":2,"id":"fetch-example","type":"function","function":{"name":"webfetch","arguments":json!({"url":page_url}).to_string()}}
                ]}),
                1 => {
                    let output: Vec<_> = body["messages"].as_array().unwrap().iter().filter(|m| m["role"] == "tool").collect();
                    assert_eq!(output.len(), 3);
                    assert!(output[0]["content"].as_str().unwrap().contains("exactly one"));
                    assert!(output[1]["content"].as_str().unwrap().contains("404"));
                    assert!(output[2]["content"].as_str().unwrap().contains("Synthetic worker page"));
                    assert!(!output[2]["content"].as_str().unwrap().contains("hidden_example_script"));
                    json!({"content":"Synthetic retrieval complete"})
                }
                _ => panic!("unexpected repeated inference"),
            };
            let chunk = json!({"choices":[{"delta":delta}]});
            ([("content-type","text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
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
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let launch = || WorkerLaunch {
        version: 1,
        identity: f.store.worker_bootstrap(&f.attempt.id).unwrap().identity,
        leader: f.url.clone(),
        bearer: f.credential.bearer().into(),
        workspace: workspace.clone(),
        protected_root: f.root.clone(),
        recovery: f.root.join("worker-recovery"),
    };
    struct FailFetchCheckpoint(std::sync::atomic::AtomicUsize);
    impl beale_storage::worker_spool::SpoolFaults for FailFetchCheckpoint {
        fn before_commit(
            &self,
            write: beale_storage::worker_spool::SpoolWrite,
        ) -> anyhow::Result<()> {
            if write == beale_storage::worker_spool::SpoolWrite::Checkpoint {
                anyhow::ensure!(
                    self.0.fetch_add(1, Ordering::SeqCst) != 4,
                    "synthetic fetch checkpoint full"
                );
            }
            Ok(())
        }
    }
    assert!(
        super::worker::run_with_faults(
            &mut launch(),
            Some(Arc::new(FailFetchCheckpoint(
                std::sync::atomic::AtomicUsize::new(0)
            )))
        )
        .await
        .is_err()
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // Recovery cannot depend on the page remaining available or unchanged.
    page_server.abort();
    let _ = page_server.await;
    run_assigned(launch()).await.unwrap();
    run_assigned(launch()).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let scan = f.store.load("scan-example", true).unwrap();
    let outcomes: Vec<_> = scan
        .events
        .iter()
        .filter(|e| e.payload["event"]["kind"] == "tool_outcome")
        .collect();
    assert_eq!(outcomes.len(), 3);
    assert_eq!(outcomes[0].payload["event"]["result"]["is_error"], true);
    assert_eq!(outcomes[1].payload["event"]["result"]["is_error"], true);
    assert_eq!(outcomes[2].payload["event"]["result"]["is_error"], false);
    assert!(
        !serde_json::to_string(&scan)
            .unwrap()
            .contains(f.credential.bearer())
    );
    let spool = beale_storage::worker_spool::WorkerSpool::open(
        &f.root.join("worker-recovery"),
        launch().identity,
    )
    .unwrap();
    assert_eq!(
        spool
            .claim(&beale_contracts::swarm_worker::WorkRequest::Environment {
                call: beale_contracts::swarm_worker::ToolInvocation {
                    id: "fetch-example".into(),
                    name: "webfetch".into(),
                    arguments: json!({"url":"https://changed.example.test"}),
                }
            })
            .unwrap_err()
            .to_string(),
        "worker operation reused with changed content"
    );
    drop(spool);
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn worker_artifact_transfer_binds_provenance_and_replays_only_durable_bytes() {
    use beale_contracts::swarm_files::{Artifact, ArtifactUpload};
    use beale_storage::swarm_files::hash;
    let f = Fixture::new().await;
    let bytes = b"Synthetic candidate evidence";
    let upload = ArtifactUpload {
        id: "artifact-worker-example".into(),
        source_path: "proof/candidate.txt".into(),
        sha256: hash(bytes),
        size: bytes.len() as u64,
        executable: Some(false),
        revision: 1,
        previous: None,
    };
    let begin = f.request(
        "begin-artifact-example",
        WorkerOperation::ArtifactBegin {
            artifact: upload.clone(),
        },
    );
    f.lose_response.store(true, Ordering::SeqCst);
    assert_eq!(
        f.client()
            .send(&begin, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Retryable
    );
    assert_eq!(
        acknowledged(
            f.client()
                .send(&begin, Duration::from_secs(5))
                .await
                .unwrap()
        )["next_offset"],
        0
    );
    let chunk = f.request(
        "chunk-artifact-example",
        WorkerOperation::ArtifactChunk {
            transfer_id: upload.id.clone(),
            offset: 0,
            bytes: bytes.to_vec(),
            sha256: hash(bytes),
        },
    );
    f.state.db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_artifact_receipt BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT, 'synthetic transfer receipt full'); END;")?; Ok(()) }).unwrap();
    assert!(
        f.client()
            .send(&chunk, Duration::from_secs(5))
            .await
            .is_err()
    );
    f.state
        .db
        .with_connection(|c| {
            assert_eq!(
                c.query_row(
                    "SELECT offset FROM swarm_file_transfers WHERE id='artifact-worker-example'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            c.execute_batch("DROP TRIGGER fail_artifact_receipt;")?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        acknowledged(
            f.client()
                .send(&chunk, Duration::from_secs(5))
                .await
                .unwrap()
        )["next_offset"],
        bytes.len()
    );
    let seal = f.request(
        "seal-artifact-example",
        WorkerOperation::ArtifactSeal {
            transfer_id: upload.id.clone(),
        },
    );
    let files = crate::research_files::store(&f.state).unwrap();
    f.state.db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_artifact_receipt BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT, 'synthetic seal receipt full'); END;")?; Ok(()) }).unwrap();
    assert!(
        f.client()
            .send(&seal, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert!(files.artifact("workspace-example", &upload.id).is_err());
    f.state
        .db
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER fail_artifact_receipt;")?;
            Ok(())
        })
        .unwrap();
    let accepted: Artifact = serde_json::from_value(acknowledged(
        f.client()
            .send(&seal, Duration::from_secs(5))
            .await
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(accepted.producer, f.deputy);
    assert_eq!(accepted.attempt, f.attempt.id);
    assert_eq!(accepted.generation, f.attempt.generation);
    assert_eq!(accepted.operation, begin.operation_id);
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .assignments
            .iter()
            .any(|a| Some(&a.id) == accepted.assignment.as_ref() && a.child_id == f.deputy)
    );
    assert_eq!(
        serde_json::from_value::<Artifact>(acknowledged(
            f.client()
                .send(&seal, Duration::from_secs(5))
                .await
                .unwrap()
        ))
        .unwrap(),
        accepted
    );
    let read = f.request(
        "read-artifact-example",
        WorkerOperation::ArtifactRead {
            artifact_id: accepted.id.clone(),
            offset: 0,
            length: 192 * 1024,
        },
    );
    let received: beale_contracts::swarm_worker::ArtifactChunkResult =
        serde_json::from_value(acknowledged(
            f.client()
                .send(&read, Duration::from_secs(5))
                .await
                .unwrap(),
        ))
        .unwrap();
    assert_eq!(received.bytes, bytes);
    assert_eq!(received.sha256, hash(bytes));
    let (attempt, credential) = f
        .store
        .start_attempt(
            "scan-example",
            &f.leader,
            &registration(),
            unix_ms(),
            unix_ms() + 60_000,
        )
        .unwrap();
    let mut foreign = chunk.clone();
    foreign.agent_id = f.leader.clone();
    foreign.attempt_id = attempt.id;
    foreign.generation = attempt.generation;
    assert!(
        WorkerTransport::new(&f.url, credential.bearer())
            .unwrap()
            .send(&foreign, Duration::from_secs(5))
            .await
            .is_err()
    );
    let mut changed = upload.clone();
    changed.sha256 = hash(b"Changed candidate");
    assert!(
        f.client()
            .send(
                &f.request(
                    "changed-begin",
                    WorkerOperation::ArtifactBegin { artifact: changed }
                ),
                Duration::from_secs(5)
            )
            .await
            .is_err()
    );
    let corrupt = f.request(
        "corrupt-chunk",
        WorkerOperation::ArtifactChunk {
            transfer_id: upload.id,
            offset: 0,
            bytes: bytes.to_vec(),
            sha256: hash(b"Wrong hash"),
        },
    );
    assert!(
        f.client()
            .send(&corrupt, Duration::from_secs(5))
            .await
            .is_err()
    );
    let mut foreign_artifact = accepted.clone();
    foreign_artifact.id = "artifact-foreign-workspace".into();
    foreign_artifact.operation = "publication-foreign-workspace".into();
    foreign_artifact.workspace = "workspace-foreign-example".into();
    files.begin_transfer(&foreign_artifact).unwrap();
    files.write_chunk(&foreign_artifact.id, 0, bytes).unwrap();
    files.seal(&foreign_artifact.id).unwrap();
    let foreign_read = f.request(
        "read-foreign-artifact",
        WorkerOperation::ArtifactRead {
            artifact_id: foreign_artifact.id,
            offset: 0,
            length: 10,
        },
    );
    assert!(
        f.client()
            .send(&foreign_read, Duration::from_secs(5))
            .await
            .is_err()
    );
    let blob = f.root.join("host/swarm-files/blobs").join(&accepted.sha256);
    std::fs::write(&blob, "Corrupted accepted bytes").unwrap();
    assert!(
        f.client()
            .send(&seal, Duration::from_secs(5))
            .await
            .is_err(),
        "a saved seal receipt cannot accept a missing or corrupt blob"
    );
    std::fs::write(&blob, bytes).unwrap();
    f.store.local_stop("scan-example", &f.deputy).unwrap();
    assert!(
        f.client()
            .send(&read, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert_eq!(
        files.artifact("workspace-example", &accepted.id).unwrap(),
        accepted
    );
    drop(files);
    f.close().await;
}

#[tokio::test]
async fn prepared_worker_maps_saved_command_policy_and_keeps_central_review_across_operating_systems()
 {
    use beale_contracts::swarm_worker::{ToolInvocation, WorkerShell};
    let shell = if cfg!(windows) {
        WorkerShell::Bash
    } else {
        WorkerShell::Powershell
    };
    let f = Fixture::with_shell(true, shell, true).await;
    let bootstrap = f.store.worker_bootstrap(&f.attempt.id).unwrap();
    assert_eq!(bootstrap.shell, shell);
    assert!(
        bootstrap
            .enabled_tools
            .iter()
            .any(|name| name == shell.tool())
    );
    assert!(
        !bootstrap
            .enabled_tools
            .iter()
            .any(|name| name == beale_tools::COMMAND_TOOL_NAME)
    );
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .config
            .enabled_tools
            .iter()
            .any(|name| name == beale_tools::COMMAND_TOOL_NAME)
    );
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| async move {
            let definitions = body["tools"].as_array().unwrap();
            let command = definitions
                .iter()
                .find(|tool| tool["function"]["name"] == shell.tool())
                .unwrap();
            assert!(
                command["function"]["parameters"]["properties"]
                    .get("machine")
                    .is_none()
            );
            assert!(
                !definitions
                    .iter()
                    .any(|tool| tool["function"]["name"] == beale_tools::COMMAND_TOOL_NAME)
            );
            let system = body["messages"][0]["content"].as_str().unwrap();
            assert!(system.contains(&format!("Assigned execution uses `{}`", shell.tool())));
            assert!(!system.contains("On this Windows host"));
            assert!(system.contains("Research-tool guidance"));
            assert_eq!(system.contains("dig ("), shell == WorkerShell::Bash);
            let chunk = json!({"choices":[{"delta":{"content":"Synthetic remote answer"}}]});
            (
                [("content-type", "text/event-stream")],
                format!("data: {chunk}\n\ndata: [DONE]\n\n"),
            )
        }),
    );
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
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let output = acknowledged(
        f.client()
            .send(
                &inference_request(&f, "cross-shell-inference"),
                Duration::from_secs(5),
            )
            .await
            .unwrap(),
    );
    assert_eq!(output["text"], "Synthetic remote answer");
    server.abort();
    let _ = server.await;
    for rule in beale_safety::rules(&f.state.db).unwrap() {
        f.state
            .db
            .save_permission_rule(&rule.id, &rule.title, &rule.instruction, true)
            .unwrap();
    }
    let server = reviewer(&f, "deputy", false).await;
    let request = f.request(
        "cross-shell-review",
        WorkerOperation::Review {
            call: ToolInvocation {
                id: "command-example".into(),
                name: shell.tool().into(),
                arguments: json!({"command":"synthetic command"}),
            },
        },
    );
    f.state.db.with_connection(|c| {
        c.execute_batch("CREATE TRIGGER fail_denial_receipt BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT, 'synthetic denial receipt full'); END;")?;
        Ok(())
    }).unwrap();
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Retryable
    );
    assert!(
        f.store
            .replay_worker_mutation(f.credential.bearer(), &request, unix_ms())
            .unwrap()
            .is_none()
    );
    f.state
        .db
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER fail_denial_receipt;")?;
            Ok(())
        })
        .unwrap();
    f.lose_response.store(true, Ordering::SeqCst);
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Retryable
    );
    server.abort();
    let _ = server.await;
    assert_eq!(
        acknowledged(
            f.client()
                .send(&request, Duration::from_secs(5))
                .await
                .unwrap()
        )["approved"],
        false
    );
    // An incorrectly placed executable rejects capability mismatch before any
    // inference or target tool dispatch, rather than substituting its host shell.
    let workspace = f.root.join("worker-workspace");
    std::fs::create_dir(&workspace).unwrap();
    let launch = WorkerLaunch {
        version: 1,
        identity: bootstrap.identity,
        leader: f.url.clone(),
        bearer: f.credential.bearer().into(),
        workspace,
        protected_root: f.root.clone(),
        recovery: f.root.join("worker-recovery"),
    };
    assert!(run_assigned(launch).await.is_err());
    assert_eq!(f.store.worker_progress(&f.attempt.id).unwrap().revision, 0);
    f.close().await;
}

#[tokio::test]
async fn host_bootstrap_uses_only_the_assigned_agents_saved_context_and_rejects_identity_changes() {
    use beale_contracts::swarm_worker::WorkerShell;
    let f = Fixture::new().await;
    let prior = f.store.worker_progress(&f.attempt.id).unwrap().turn;
    let model =
        super::gateway::model_for_agent(&f.store, "scan-example", &f.deputy, &prior.profile)
            .unwrap();
    let save_context = |session: &str, content: &str| {
        let row = beale_storage::SessionRow {
            id: session.into(),
            workspace_id: "workspace-example".into(),
            title: "Example".into(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        f.state.db.append_turn_with_metadata(&row, &beale_storage::TurnRow { number: 1, provider: model.provider, method: model.method, model: model.model.clone(), prompt: "Prior synthetic task".into(), answer: "Prior synthetic answer".into(), duration_ms: 0, internal_prompt: false }, true,
            beale_storage::TurnActivity { tools: &[], reasoning: &[], commentary: &[] }, None,
            beale_storage::TurnMetadata { provider_context_json: Some(&json!([{"role":"system","content":"Old runtime policy"},{"role":"assistant","content":content}]).to_string()), agent_role: Some(&prior.profile), ..Default::default() }).unwrap();
    };
    save_context(&prior.session_id, "Own synthetic retained context");
    save_context("scan-example", "Ancestor conversation must stay private");
    f.store
        .local_finish(&prior, json!({"saved":true}), "Synthetic completion", false)
        .unwrap();
    f.store
        .local_message(
            "scan-example",
            "resume-example",
            &f.deputy,
            "Related synthetic follow-up",
        )
        .unwrap();
    let turn = f
        .store
        .local_claim("scan-example", &f.deputy, unix_ms(), 64)
        .unwrap()
        .unwrap();
    for change in 0..3 {
        let mut invalid = turn.clone();
        match change {
            0 => invalid.session_id = "scan-example".into(),
            1 => invalid.role = Role::Leader,
            _ => invalid.profile = "wrong-profile".into(),
        }
        assert!(
            prepare_worker(
                &f.state.db,
                &invalid,
                WorkerShell::native(),
                &registration(),
                unix_ms()
            )
            .is_err()
        );
    }
    let (bootstrap, _) = prepare_worker(
        &f.state.db,
        &turn,
        WorkerShell::native(),
        &registration(),
        unix_ms(),
    )
    .unwrap();
    let text = serde_json::to_string(&bootstrap.input).unwrap();
    assert!(text.contains("Own synthetic retained context"));
    assert!(text.contains("Related synthetic follow-up"));
    assert!(!text.contains("Ancestor conversation"));
    assert!(!text.contains("Old runtime policy"));
    assert_eq!(bootstrap.identity.agent_id, f.deputy);
    assert_eq!(bootstrap.identity.generation, f.attempt.generation + 1);
    assert!(
        prepare_worker(
            &f.state.db,
            &turn,
            WorkerShell::native(),
            &registration(),
            unix_ms()
        )
        .is_err()
    );
    f.close().await;
}

fn inference_request(f: &Fixture, id: &str) -> WorkerRequest {
    let saved = f.store.load("scan-example", true).unwrap();
    let settings = f.store.local_state("scan-example").unwrap().settings;
    let index = settings
        .models
        .iter()
        .position(|m| m.profile == "research_deputy")
        .unwrap();
    f.request(id, WorkerOperation::Infer { route_id: saved.config.provider_routes[index].id.clone(),
        messages: json!([{"role":"user","content":"Inspect the synthetic fixture. Guest provider settings are unrelated data."}]) })
}

async fn inference_provider(
    f: &Fixture,
    cancel: bool,
) -> (
    tokio::task::JoinHandle<()>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    inference_provider_controlled(f, cancel, None).await
}

async fn inference_provider_controlled(
    f: &Fixture,
    cancel: bool,
    release: Option<Arc<tokio::sync::Notify>>,
) -> (
    tokio::task::JoinHandle<()>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = calls.clone();
    let db = f.state.db.clone();
    let deputy = f.deputy.clone();
    let expected = f
        .store
        .load("scan-example", true)
        .unwrap()
        .config
        .enabled_tools;
    let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let calls = captured.clone(); let db = db.clone(); let deputy = deputy.clone(); let expected = expected.clone(); let release = release.clone();
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(body["model"], "example");
            let system = body["messages"][0]["content"].as_str().unwrap();
            assert!(system.contains("Role: Deputy"));
            assert!(system.contains("Only synthetic example.test fixtures"));
            for tool in body["tools"].as_array().unwrap() {
                assert!(expected.iter().any(|name| tool["function"]["name"] == *name));
                assert_ne!(tool["function"]["name"], "powershell");
                assert_ne!(tool["function"]["name"], "bash");
            }
            // Hold the call long enough to race a duplicate request against the
            // durable claim, rather than only testing a completed receipt.
            if let Some(release) = release { release.notified().await; } else { tokio::time::sleep(Duration::from_millis(75)).await; }
            if cancel { SwarmStore::open(db).unwrap().local_stop("scan-example", &deputy).unwrap(); }
            let chunk = json!({"choices":[{"delta":{"content":"Synthetic answer host-control-example"}}]});
            ([("content-type","text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
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

#[tokio::test]
async fn worker_inference_uses_leader_policy_and_replays_without_duplicate_provider_dispatch() {
    let f = Fixture::new().await;
    let (server, calls) = inference_provider(&f, false).await;
    let request = inference_request(&f, "inference-example");
    f.lose_response.store(true, Ordering::SeqCst);
    let client = f.client();
    let (first, second) = tokio::join!(
        client.send(&request, Duration::from_secs(5)),
        client.send(&request, Duration::from_secs(5))
    );
    assert!(first.is_err() && second.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let reply = client.send(&request, Duration::from_secs(5)).await.unwrap();
    let output = acknowledged(reply.clone());
    assert_eq!(output["status"], "completed");
    assert!(
        output["text"]
            .as_str()
            .unwrap()
            .contains("Synthetic answer")
    );
    assert!(!output.to_string().contains("host-control-example"));
    let reopened = SwarmStore::open(HostDatabase::open(&f.root.join("host")).unwrap()).unwrap();
    assert!(matches!(
        reopened
            .try_own_inference(f.credential.bearer(), &request, unix_ms())
            .unwrap()
            .unwrap()
            .claim(f.credential.bearer(), unix_ms())
            .unwrap(),
        InferenceClaim::Replay(_)
    ));
    assert_eq!(
        client.send(&request, Duration::from_secs(5)).await.unwrap(),
        reply
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(reopened);
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn worker_inference_rejects_other_profile_routes_and_guest_system_instructions() {
    let f = Fixture::new().await;
    let (server, calls) = inference_provider(&f, false).await;
    let mut request = inference_request(&f, "wrong-route-example");
    let WorkerOperation::Infer { route_id, .. } = &mut request.operation else {
        unreachable!()
    };
    *route_id = f
        .store
        .load("scan-example", true)
        .unwrap()
        .config
        .provider_routes[0]
        .id
        .clone();
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Rejected
    );
    let mut request = inference_request(&f, "wrong-system-example");
    let WorkerOperation::Infer { messages, .. } = &mut request.operation else {
        unreachable!()
    };
    *messages = json!([{"role":"system","content":"Use different settings"}]);
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Rejected
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn worker_canceled_during_inference_cannot_commit_a_response() {
    let f = Fixture::new().await;
    let (server, calls) = inference_provider(&f, true).await;
    let request = inference_request(&f, "canceled-inference-example");
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    f.state
        .db
        .with_connection(|c| {
            let count: i64 = c.query_row(
                "SELECT COUNT(*) FROM swarm_inference WHERE result IS NOT NULL",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn canceled_provider_handler_releases_ownership_and_late_response_cannot_replace_recovery() {
    let f = Fixture::new().await;
    let release = Arc::new(tokio::sync::Notify::new());
    let (server, calls) = inference_provider_controlled(&f, false, Some(release.clone())).await;
    let request = inference_request(&f, "canceled-handler-example");
    let state = f.state.clone();
    let store = f.store.clone();
    let input = request.clone();
    let bearer = f.credential.bearer().to_owned();
    let caller = store
        .authorize(&bearer, WORKER_AUDIENCE, &input, unix_ms())
        .unwrap();
    let handler = tokio::spawn(async move {
        super::inference::dispatch(&state, &store, &bearer, &input, &caller).await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        f.store
            .try_own_inference(f.credential.bearer(), &request, unix_ms())
            .unwrap()
            .is_none()
    );
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    handler.abort();
    assert!(handler.await.unwrap_err().is_cancelled());
    let reply = f
        .client()
        .send(&request, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        acknowledged(reply.clone()),
        json!({"status":"unavailable","retryable":true})
    );
    release.notify_one();
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
        reply
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let owner = f
        .store
        .try_own_inference(f.credential.bearer(), &request, unix_ms())
        .unwrap()
        .unwrap();
    let claim: String = f
        .state
        .db
        .with_connection(|db| {
            Ok(db.query_row(
                "SELECT claim FROM swarm_inference WHERE operation_id='canceled-handler-example'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    let late = json!({"status":"completed","text":"Late synthetic result","calls":[],"reasoning_summaries":[],"continuation":[],"usage":null,"response_id":null});
    assert!(
        owner
            .complete(f.credential.bearer(), &claim, late, unix_ms())
            .is_err()
    );
    drop(owner);
    f.store
        .fence_attempt("scan-example", &f.attempt.id, true)
        .unwrap();
    let (replacement, credential) = f
        .store
        .start_attempt(
            "scan-example",
            &f.deputy,
            &registration(),
            unix_ms(),
            unix_ms() + 60_000,
        )
        .unwrap();
    let mut rebound = request.clone();
    rebound.attempt_id = replacement.id;
    rebound.generation = replacement.generation;
    assert!(
        f.store
            .try_own_inference(credential.bearer(), &rebound, unix_ms())
            .is_err()
    );
    assert!(
        f.store
            .replay_worker_mutation(credential.bearer(), &rebound, unix_ms())
            .is_err()
    );
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[test]
#[ignore = "subprocess fixture invoked by inference recovery acceptance"]
fn inference_owner_process_fixture() {
    let root = std::env::current_dir().unwrap();
    let input: Value =
        serde_json::from_slice(&std::fs::read(root.join("inference-fixture.json")).unwrap())
            .unwrap();
    let request: WorkerRequest = serde_json::from_value(input["request"].clone()).unwrap();
    let store = SwarmStore::open(HostDatabase::open(&root.join("host")).unwrap()).unwrap();
    let owner = store
        .try_own_inference(input["bearer"].as_str().unwrap(), &request, unix_ms())
        .unwrap()
        .unwrap();
    assert!(matches!(
        owner
            .claim(input["bearer"].as_str().unwrap(), unix_ms())
            .unwrap(),
        InferenceClaim::Dispatch(_)
    ));
    std::fs::write(root.join("inference-owned"), "ready").unwrap();
    loop {
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[tokio::test]
async fn abandoned_inference_recovers_after_process_death_without_redispatch_and_warns_once() {
    let f = Fixture::new().await;
    let (server, calls) = inference_provider(&f, false).await;
    let request = inference_request(&f, "abandoned-inference-example");
    std::fs::write(
        f.root.join("inference-fixture.json"),
        serde_json::to_vec(&json!({"request":request,"bearer":f.credential.bearer()})).unwrap(),
    )
    .unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "distributed::gateway_tests::inference_owner_process_fixture",
        ])
        .current_dir(&f.root)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f.root.join("inference-owned").exists() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "inference fixture exited before owning dispatch"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        f.store
            .try_own_inference(f.credential.bearer(), &request, unix_ms())
            .unwrap()
            .is_none()
    );
    let concurrent = inference_request(&f, "concurrent-inference-example");
    assert!(
        f.store
            .try_own_inference(f.credential.bearer(), &concurrent, unix_ms())
            .unwrap()
            .is_none()
    );
    let (other_attempt, other_credential) = f
        .store
        .start_attempt(
            "scan-example",
            &f.leader,
            &registration(),
            unix_ms(),
            unix_ms() + 60_000,
        )
        .unwrap();
    let mut other = request.clone();
    other.agent_id = f.leader.clone();
    other.attempt_id = other_attempt.id;
    other.generation = other_attempt.generation;
    let other_owner = f
        .store
        .try_own_inference(other_credential.bearer(), &other, unix_ms())
        .unwrap()
        .unwrap();
    drop(other_owner);
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    let owner = f
        .store
        .try_own_inference(f.credential.bearer(), &request, unix_ms())
        .unwrap()
        .unwrap();
    assert!(matches!(
        owner.claim(f.credential.bearer(), unix_ms()).unwrap(),
        InferenceClaim::Pending
    ));
    f.state.db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_recovery_warning BEFORE UPDATE ON swarm_scans BEGIN SELECT RAISE(ABORT, 'synthetic warning full'); END;")?; Ok(()) }).unwrap();
    assert!(owner.recover(f.credential.bearer(), unix_ms()).is_err());
    assert!(
        f.store
            .replay_worker_mutation(f.credential.bearer(), &request, unix_ms())
            .unwrap()
            .is_none()
    );
    f.state.db.with_connection(|c| {
        let result: Option<String> = c.query_row("SELECT result FROM swarm_inference WHERE operation_id='abandoned-inference-example'", [], |r| r.get(0))?;
        assert!(result.is_none());
        c.execute_batch("DROP TRIGGER fail_recovery_warning; CREATE TRIGGER fail_recovery_receipt BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT, 'synthetic receipt full'); END;")?;
        Ok(())
    }).unwrap();
    assert!(owner.recover(f.credential.bearer(), unix_ms()).is_err());
    assert!(
        f.store
            .replay_worker_mutation(f.credential.bearer(), &request, unix_ms())
            .unwrap()
            .is_none()
    );
    f.state
        .db
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER fail_recovery_receipt;")?;
            Ok(())
        })
        .unwrap();
    drop(owner);
    let reply = f
        .client()
        .send(&request, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        acknowledged(reply.clone()),
        json!({"status":"unavailable","retryable":true})
    );
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
        reply
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let scan = f.store.load("scan-example", true).unwrap();
    let warnings: Vec<_> = scan
        .events
        .iter()
        .filter(|e| e.kind == "inference-interrupted")
        .collect();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].agent_id, f.deputy);
    assert_eq!(warnings[0].payload["attempt_id"], f.attempt.id);
    assert_eq!(warnings[0].payload["generation"], f.attempt.generation);
    let mut changed = request.clone();
    if let WorkerOperation::Infer { messages, .. } = &mut changed.operation {
        *messages = json!([{"role":"user","content":"Changed synthetic request"}]);
    }
    assert!(
        f.client()
            .send(&changed, Duration::from_secs(5))
            .await
            .is_err()
    );
    let next = inference_request(&f, "fresh-inference-example");
    assert_eq!(
        acknowledged(
            f.client()
                .send(&next, Duration::from_secs(5))
                .await
                .unwrap()
        )["status"],
        "completed"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    f.store.local_stop("scan-example", &f.deputy).unwrap();
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert!(
        f.store
            .try_own_inference(f.credential.bearer(), &next, unix_ms())
            .is_err()
    );
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn resumed_worker_reconciles_pending_inference_before_advancing_its_model_loop() {
    for saved_result in [false, true] {
        let f = Fixture::new().await;
        let (server, calls) = inference_provider(&f, false).await;
        let bootstrap = f.store.worker_bootstrap(&f.attempt.id).unwrap();
        let request = f.request(
            "resume-inference-example",
            WorkerOperation::Infer {
                route_id: bootstrap.route_id.clone(),
                messages: json!(bootstrap.input),
            },
        );
        let owner = f
            .store
            .try_own_inference(f.credential.bearer(), &request, unix_ms())
            .unwrap()
            .unwrap();
        let InferenceClaim::Dispatch(claim) =
            owner.claim(f.credential.bearer(), unix_ms()).unwrap()
        else {
            panic!("expected initial dispatch");
        };
        if saved_result {
            f.state.db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_resume_receipt BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT, 'synthetic receipt full'); END;")?; Ok(()) }).unwrap();
            let result = json!({"status":"completed","text":"Synthetic retained answer","calls":[],"reasoning_summaries":[],"continuation":[],"usage":null,"response_id":null});
            assert!(
                owner
                    .complete(f.credential.bearer(), &claim, result, unix_ms())
                    .is_err()
            );
            f.state
                .db
                .with_connection(|c| {
                    c.execute_batch("DROP TRIGGER fail_resume_receipt;")?;
                    Ok(())
                })
                .unwrap();
        }
        drop(owner);
        let workspace = f.root.join("worker-workspace");
        std::fs::create_dir(&workspace).unwrap();
        let recovery = f.root.join("worker-recovery");
        let spool =
            beale_storage::worker_spool::WorkerSpool::open(&recovery, bootstrap.identity.clone())
                .unwrap();
        spool
            .checkpoint(
                "retained-loop-example",
                0,
                &json!({
                    "version":2, "active_elapsed_ms":86_400_001, "shell":bootstrap.shell, "route_id":bootstrap.route_id,
                    "format":bootstrap.message_format, "enabled_tools":bootstrap.enabled_tools,
                    "input":bootstrap.input, "provider_failures":0,
                    "phase":{"phase":"infer","operation_id":request.operation_id},
                }),
            )
            .unwrap();
        drop(spool);
        let launch = || WorkerLaunch {
            version: 1,
            identity: bootstrap.identity.clone(),
            leader: f.url.clone(),
            bearer: f.credential.bearer().into(),
            workspace: workspace.clone(),
            protected_root: f.root.clone(),
            recovery: recovery.clone(),
        };
        run_assigned(launch()).await.unwrap();
        let elapsed = f
            .store
            .worker_progress(&f.attempt.id)
            .unwrap()
            .active_elapsed_ms;
        assert!(elapsed >= 86_400_001);
        run_assigned(launch()).await.unwrap();
        assert_eq!(
            f.store
                .worker_progress(&f.attempt.id)
                .unwrap()
                .active_elapsed_ms,
            elapsed
        );
        assert_eq!(calls.load(Ordering::SeqCst), usize::from(!saved_result));
        let progress = f.store.worker_progress(&f.attempt.id).unwrap();
        assert!(progress.answer.as_deref().unwrap().contains("Synthetic"));
        let warnings = f
            .store
            .load("scan-example", true)
            .unwrap()
            .events
            .into_iter()
            .filter(|e| e.kind == "inference-interrupted")
            .count();
        assert_eq!(warnings, usize::from(!saved_result));
        server.abort();
        let _ = server.await;
        f.close().await;
    }
}

#[tokio::test]
async fn inference_result_survives_receipt_failure_and_gateway_retry_never_calls_provider_again() {
    let f = Fixture::new().await;
    let (server, calls) = inference_provider(&f, false).await;
    let request = inference_request(&f, "retained-inference-example");
    f.state.db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_provider_receipt BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT, 'synthetic inference receipt full'); END;")?; Ok(()) }).unwrap();
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        f.store
            .replay_worker_mutation(f.credential.bearer(), &request, unix_ms())
            .unwrap()
            .is_none()
    );
    f.state
        .db
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER fail_provider_receipt;")?;
            Ok(())
        })
        .unwrap();
    server.abort();
    let _ = server.await;
    let reply = f
        .client()
        .send(&request, Duration::from_secs(5))
        .await
        .unwrap();
    let result = acknowledged(reply.clone());
    assert_eq!(result["status"], "completed");
    assert!(
        result["text"]
            .as_str()
            .unwrap()
            .contains("Synthetic answer")
    );
    assert!(!result.to_string().contains("host-control-example"));
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
        reply
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        !f.store
            .load("scan-example", true)
            .unwrap()
            .events
            .iter()
            .any(|e| e.kind == "inference-interrupted")
    );
    f.close().await;
}

#[tokio::test]
async fn incomplete_inference_claim_survives_reopen_and_receipt_failure_without_redispatch() {
    let f = Fixture::new().await;
    let request = inference_request(&f, "incomplete-inference-example");
    let owner = f
        .store
        .try_own_inference(f.credential.bearer(), &request, unix_ms())
        .unwrap()
        .unwrap();
    let InferenceClaim::Dispatch(claim) = owner.claim(f.credential.bearer(), unix_ms()).unwrap()
    else {
        panic!("expected claim")
    };
    let reopened = SwarmStore::open(HostDatabase::open(&f.root.join("host")).unwrap()).unwrap();
    assert!(
        reopened
            .try_own_inference(f.credential.bearer(), &request, unix_ms())
            .unwrap()
            .is_none()
    );
    drop(owner);
    let owner = reopened
        .try_own_inference(f.credential.bearer(), &request, unix_ms())
        .unwrap()
        .unwrap();
    assert!(matches!(
        owner.claim(f.credential.bearer(), unix_ms()).unwrap(),
        InferenceClaim::Pending
    ));
    let result = json!({"status":"completed","text":"Synthetic retained result","calls":[],"reasoning_summaries":[],"continuation":[],"usage":null,"response_id":null});
    let mut reused = request.clone();
    reused.operation = WorkerOperation::MessageParent {
        message: "Must not reuse an inference identity".into(),
    };
    assert!(
        f.client()
            .send(&reused, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert!(
        !f.store
            .local_state("scan-example")
            .unwrap()
            .inbox
            .iter()
            .any(|message| message.id == request.operation_id)
    );
    assert!(
        owner
            .complete(
                f.credential.bearer(),
                "foreign-claim",
                result.clone(),
                unix_ms()
            )
            .is_err()
    );
    f.state.db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_inference_receipt BEFORE INSERT ON swarm_receipts BEGIN SELECT RAISE(ABORT, 'synthetic inference disk full'); END;")?; Ok(()) }).unwrap();
    assert!(
        owner
            .complete(f.credential.bearer(), &claim, result.clone(), unix_ms())
            .is_err()
    );
    assert!(matches!(
        owner.claim(f.credential.bearer(), unix_ms()).unwrap(),
        InferenceClaim::Pending
    ));
    f.state
        .db
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER fail_inference_receipt;")?;
            Ok(())
        })
        .unwrap();
    drop(owner);
    let owner = reopened
        .try_own_inference(f.credential.bearer(), &request, unix_ms())
        .unwrap()
        .unwrap();
    let ack = owner.recover(f.credential.bearer(), unix_ms()).unwrap();
    assert_eq!(ack.result, result);
    assert_eq!(
        owner
            .complete(f.credential.bearer(), &claim, result, unix_ms())
            .unwrap(),
        ack
    );
    assert!(
        owner
            .complete(f.credential.bearer(), &claim, json!({}), unix_ms())
            .is_err()
    );
    drop(owner);
    drop(reopened);
    f.close().await;
}

#[cfg(feature = "findings")]
#[tokio::test]
async fn remote_finding_lost_response_replays_one_canonical_record() {
    use beale_findings::FindingStore;
    let f = Fixture::new().await;
    let request = f.request("finding-example", WorkerOperation::Canonical { tool: "findings_create".into(), arguments: json!({"title":"Synthetic candidate","summary":"Candidate observation","hypothesis":"A synthetic input may violate an example invariant","agent_role":"leader"}) });
    f.lose_response.store(true, Ordering::SeqCst);
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Retryable
    );
    assert!(
        f.state
            .db
            .finding_get("workspace-example", "F1")
            .unwrap()
            .is_some()
    );
    let saved = f
        .client()
        .send(&request, Duration::from_secs(5))
        .await
        .unwrap();
    // Replay remains independent of provider/reviewer availability after acceptance.
    for rule in beale_safety::rules(&f.state.db).unwrap() {
        f.state
            .db
            .save_permission_rule(&rule.id, &rule.title, &rule.instruction, true)
            .unwrap();
    }
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
        saved
    );
    assert!(
        f.state
            .db
            .finding_get("workspace-example", "F2")
            .unwrap()
            .is_none()
    );
    f.close().await;
}

#[tokio::test]
async fn deputy_remote_spawn_and_messages_share_central_hierarchy_and_retry_identity() {
    let f = Fixture::new().await;
    f.allocation_snapshot();
    let request = f.request(
        "spawn-worker-example",
        WorkerOperation::Spawn {
            profile: ProfileRef {
                id: "investigator".into(),
                revision: 1,
            },
            task: task(),
        },
    );
    f.lose_response.store(true, Ordering::SeqCst);
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Retryable
    );
    let result = acknowledged(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    let child = result["child_id"].as_str().unwrap();
    let all = f.store.summaries("scan-example").unwrap();
    assert_eq!(all.len(), 3);
    assert!(all.iter().any(|a| a.id == child
        && a.parent_id.as_deref() == Some(f.deputy.as_str())
        && a.role == Role::Worker));
    assert!(
        all.iter()
            .any(|a| a.id == f.deputy && a.parent_id.as_deref() == Some(f.leader.as_str()))
    );
    let list = f.request(
        "list-example",
        WorkerOperation::Canonical {
            tool: "swarm_list_subagents".into(),
            arguments: json!({}),
        },
    );
    assert_eq!(
        acknowledged(
            f.client()
                .send(&list, Duration::from_secs(5))
                .await
                .unwrap()
        )
        .as_array()
        .unwrap()
        .len(),
        1
    );
    let message = f.request(
        "message-example",
        WorkerOperation::MessageParent {
            message: "Synthetic candidate retained".into(),
        },
    );
    let reply = f
        .client()
        .send(&message, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        f.client()
            .send(&message, Duration::from_secs(5))
            .await
            .unwrap(),
        reply
    );
    assert_eq!(
        f.store
            .local_state("scan-example")
            .unwrap()
            .inbox
            .iter()
            .filter(|m| m.id == "message-example")
            .count(),
        1
    );
    f.close().await;
}

#[tokio::test]
async fn worker_gateway_rejects_identity_escalation_host_control_and_canceled_reconnect() {
    let f = Fixture::new().await;
    let request = f.request("heartbeat-example", WorkerOperation::Heartbeat {});
    assert!(matches!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
        WorkerReply::Lease { .. }
    ));
    for field in 0..5 {
        let mut changed = request.clone();
        match field {
            0 => changed.workspace_id = "other-workspace".into(),
            1 => changed.scan_id = "other-scan".into(),
            2 => changed.agent_id = f.leader.clone(),
            3 => changed.attempt_id = "other-attempt".into(),
            _ => changed.generation += 1,
        }
        assert_eq!(
            f.client()
                .send(&changed, Duration::from_secs(5))
                .await
                .unwrap_err()
                .kind,
            TransportFailure::Rejected
        );
    }
    let foreign_parent = f.request(
        "impersonate-parent",
        WorkerOperation::MessageChild {
            child_id: f.leader.clone(),
            message: "Not authorized".into(),
        },
    );
    assert_eq!(
        f.client()
            .send(&foreign_parent, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Rejected
    );
    let admin = reqwest::Client::new()
        .get(format!("{}{}", f.url, beale_contracts::SESSIONS_PATH))
        .bearer_auth(f.credential.bearer())
        .send()
        .await
        .unwrap();
    assert_eq!(admin.status(), StatusCode::UNAUTHORIZED);
    f.store.local_stop("scan-example", &f.deputy).unwrap();
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Rejected
    );
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .attempts
            .iter()
            .any(|a| a.id == f.attempt.id && a.state == ExecutionState::Running)
    );
    f.store
        .fence_attempt("scan-example", &f.attempt.id, true)
        .unwrap();
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Rejected
    );
    assert!(
        f.store
            .start_attempt(
                "scan-example",
                &f.deputy,
                &registration(),
                unix_ms(),
                unix_ms() + 60_000
            )
            .is_err()
    );
    f.close().await;
}

fn registration() -> WorkerRegistration {
    WorkerRegistration {
        version: VERSION,
        audience: WORKER_AUDIENCE.into(),
        capabilities: vec![
            WorkerCapability::CanonicalRouting,
            WorkerCapability::ArtifactTransfer,
            WorkerCapability::DurableEvents,
        ],
    }
}

#[tokio::test]
async fn worker_gateway_rejects_expired_and_revoked_credentials_without_branch_stop() {
    let f = Fixture::new().await;
    let now = unix_ms();
    let (attempt, credential) = f
        .store
        .start_attempt(
            "scan-example",
            &f.leader,
            &registration(),
            now - 2000,
            now - 1000,
        )
        .unwrap();
    let mut request = f.request("expired-heartbeat-example", WorkerOperation::Heartbeat {});
    request.agent_id = f.leader.clone();
    request.attempt_id = attempt.id;
    request.generation = attempt.generation;
    let expired = WorkerTransport::new(&f.url, credential.bearer()).unwrap();
    assert_eq!(
        expired
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Rejected
    );
    let heartbeat = f.request("revoked-heartbeat-example", WorkerOperation::Heartbeat {});
    f.store
        .fence_attempt("scan-example", &f.attempt.id, false)
        .unwrap();
    assert_eq!(
        f.client()
            .send(&heartbeat, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Rejected
    );
    assert!(
        !f.store
            .local_state("scan-example")
            .unwrap()
            .controls
            .iter()
            .any(|c| c.stop_requested)
    );
    assert!(
        f.store
            .start_attempt(
                "scan-example",
                &f.deputy,
                &registration(),
                unix_ms(),
                unix_ms() + 60_000
            )
            .is_err()
    );
    f.close().await;
}

#[tokio::test]
async fn worker_spawn_receipt_failure_rolls_back_assignment_and_both_dispatch_receipts() {
    let f = Fixture::new().await;
    f.allocation_snapshot();
    // Let the shared local dispatcher save its result, then fail the outer
    // authenticated receipt. Neither that result nor its assignment may survive.
    f.state.db.with_connection(|c| {
        c.execute_batch("CREATE TRIGGER fail_outer_worker_receipt BEFORE INSERT ON swarm_receipts WHEN NEW.actor NOT LIKE 'local-%' BEGIN SELECT RAISE(ABORT, 'synthetic receipt disk full'); END;")?;
        Ok(())
    }).unwrap();
    let request = f.request(
        "spawn-receipt-example",
        WorkerOperation::Spawn {
            profile: ProfileRef {
                id: "investigator".into(),
                revision: 1,
            },
            task: task(),
        },
    );
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Retryable
    );
    assert_eq!(f.store.summaries("scan-example").unwrap().len(), 2);
    assert!(
        f.store
            .replay_worker_mutation(f.credential.bearer(), &request, unix_ms())
            .unwrap()
            .is_none()
    );
    f.state
        .db
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER fail_outer_worker_receipt;")?;
            Ok(())
        })
        .unwrap();
    let accepted = f
        .client()
        .send(&request, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
        accepted
    );
    assert_eq!(f.store.summaries("scan-example").unwrap().len(), 3);
    let mut changed = request.clone();
    let WorkerOperation::Spawn { task, .. } = &mut changed.operation else {
        unreachable!()
    };
    task.question = "A different synthetic assignment".into();
    assert!(
        f.client()
            .send(&changed, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert_eq!(f.store.summaries("scan-example").unwrap().len(), 3);
    f.close().await;
}

#[tokio::test]
async fn worker_dedicated_operations_cannot_bypass_the_saved_tool_list() {
    let f = Fixture::with_tools(false).await;
    for operation in [
        WorkerOperation::Spawn {
            profile: ProfileRef {
                id: "investigator".into(),
                revision: 1,
            },
            task: task(),
        },
        WorkerOperation::MessageParent {
            message: "Synthetic result".into(),
        },
        WorkerOperation::MessageChild {
            child_id: f.leader.clone(),
            message: "Synthetic message".into(),
        },
        WorkerOperation::StopChild {
            child_id: f.leader.clone(),
        },
        WorkerOperation::Canonical {
            tool: "swarm_list_subagents".into(),
            arguments: json!({}),
        },
    ] {
        assert_eq!(
            f.client()
                .send(
                    &f.request("unavailable-example", operation),
                    Duration::from_secs(5)
                )
                .await
                .unwrap_err()
                .kind,
            TransportFailure::Rejected
        );
    }
    assert_eq!(f.store.summaries("scan-example").unwrap().len(), 2);
    assert!(matches!(
        f.client()
            .send(
                &f.request("heartbeat-example", WorkerOperation::Heartbeat {}),
                Duration::from_secs(5)
            )
            .await
            .unwrap(),
        WorkerReply::Lease { .. }
    ));
    f.close().await;
}

async fn reviewer(f: &Fixture, role: &'static str, cancel: bool) -> tokio::task::JoinHandle<()> {
    let db = f.state.db.clone();
    let deputy = f.deputy.clone();
    let rule = f.store.load("scan-example", true).unwrap().config;
    let ScanMode::Swarm { snapshot } = rule.mode else {
        panic!("expected swarm")
    };
    let instruction = snapshot.leader_rule.instruction;
    let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let db = db.clone(); let deputy = deputy.clone(); let instruction = instruction.clone();
        async move {
            let system = body["messages"][0]["content"].as_str().unwrap();
            assert!(system.contains(&format!("Trusted host role: {role}")));
            assert_eq!(system.contains(&instruction), role == "leader");
            let input: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
            assert_eq!(input["request"]["workspace_context"], "Only synthetic example.test fixtures");
            assert_eq!(input["request"]["trusted_agent"]["role"], role);
            if cancel { SwarmStore::open(db).unwrap().local_stop("scan-example", &deputy).unwrap(); }
            let decision = if cancel { "approved" } else { "denied" };
            let chunk = json!({"choices":[{"delta":{"content":json!({"decision":decision,"reason":"Synthetic review decision"}).to_string()}}]});
            ([("content-type", "text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
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
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    })
}

#[tokio::test]
async fn worker_leader_review_uses_the_saved_rule_even_with_global_rules_disabled() {
    let f = Fixture::new().await;
    let server = reviewer(&f, "leader", false).await;
    let (attempt, credential) = f
        .store
        .start_attempt(
            "scan-example",
            &f.leader,
            &registration(),
            unix_ms(),
            unix_ms() + 60_000,
        )
        .unwrap();
    let mut request = f.request(
        "leader-list-example",
        WorkerOperation::Canonical {
            tool: "swarm_list_subagents".into(),
            arguments: json!({"role":"worker"}),
        },
    );
    request.agent_id = f.leader.clone();
    request.attempt_id = attempt.id;
    request.generation = attempt.generation;
    let client = WorkerTransport::new(&f.url, credential.bearer()).unwrap();
    let reply = client.send(&request, Duration::from_secs(5)).await.unwrap();
    assert_eq!(acknowledged(reply.clone())["is_error"], true);
    server.abort();
    let _ = server.await;
    assert_eq!(
        client.send(&request, Duration::from_secs(5)).await.unwrap(),
        reply
    );
    let reopened = SwarmStore::open(HostDatabase::open(&f.root.join("host")).unwrap()).unwrap();
    assert_eq!(
        reopened
            .replay_worker_mutation(credential.bearer(), &request, unix_ms())
            .unwrap()
            .unwrap()
            .result["is_error"],
        true
    );
    let mut unavailable = request.clone();
    unavailable.operation_id = "review-service-unavailable-example".into();
    assert_eq!(
        client
            .send(&unavailable, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Retryable
    );
    assert!(
        reopened
            .replay_worker_mutation(credential.bearer(), &unavailable, unix_ms())
            .unwrap()
            .is_none()
    );
    drop(reopened);
    f.close().await;
}

#[cfg(feature = "findings")]
#[tokio::test]
async fn worker_canceled_while_reviewing_cannot_commit_a_finding() {
    use beale_findings::FindingStore;
    let f = Fixture::new().await;
    for rule in beale_safety::rules(&f.state.db).unwrap() {
        f.state
            .db
            .save_permission_rule(&rule.id, &rule.title, &rule.instruction, true)
            .unwrap();
    }
    let server = reviewer(&f, "deputy", true).await;
    let request = f.request("canceled-finding-example", WorkerOperation::Canonical { tool: "findings_create".into(), arguments: json!({"title":"Synthetic candidate", "summary":"Candidate observation", "hypothesis":"Synthetic invariant", "agent_role":"leader"}) });
    assert!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .is_err()
    );
    assert!(
        f.state
            .db
            .finding_get("workspace-example", "F1")
            .unwrap()
            .is_none()
    );
    assert!(
        f.store
            .local_state("scan-example")
            .unwrap()
            .controls
            .iter()
            .any(|c| c.agent_id == f.deputy && c.stop_requested)
    );
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn worker_warnings_commit_before_ack_and_survive_reopen_without_a_desktop() {
    let f = Fixture::new().await;
    let request = f.request("warning-example", WorkerOperation::Event { event_id: "event-example".into(), payload: json!({"kind":"warning","message":"Synthetic storage pressure","detail":"x".repeat(24000)}) });
    f.state.db.with_connection(|c| { c.execute_batch("CREATE TRIGGER fail_worker_warning BEFORE UPDATE ON swarm_scans BEGIN SELECT RAISE(ABORT, 'synthetic warning disk full'); END;")?; Ok(()) }).unwrap();
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap_err()
            .kind,
        TransportFailure::Retryable
    );
    assert!(
        !f.store
            .load("scan-example", true)
            .unwrap()
            .events
            .iter()
            .any(|e| e.id == "event-example")
    );
    f.state
        .db
        .with_connection(|c| {
            c.execute_batch("DROP TRIGGER fail_worker_warning;")?;
            Ok(())
        })
        .unwrap();
    let result = f
        .client()
        .send(&request, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        f.client()
            .send(&request, Duration::from_secs(5))
            .await
            .unwrap(),
        result
    );
    let reopened = SwarmStore::open(HostDatabase::open(&f.root.join("host")).unwrap()).unwrap();
    let state = reopened.load("scan-example", true).unwrap();
    let events: Vec<_> = state
        .events
        .iter()
        .filter(|e| e.id == "event-example")
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload["attempt_id"], f.attempt.id);
    assert_eq!(events[0].agent_id, f.deputy);
    drop(reopened);
    f.close().await;
}

#[cfg(feature = "runbooks")]
#[path = "runbooks_tests.rs"]
mod assigned_runbooks;

#[cfg(feature = "findings")]
#[path = "findings_tests.rs"]
mod assigned_findings;

#[path = "plugins_tests.rs"]
mod plugins;

#[path = "canonical_error_tests.rs"]
mod canonical_errors;

#[path = "peer_host_tests.rs"]
mod peer_hosts;

#[tokio::test]
async fn local_scheduler_reuses_workspace_and_scopes_receipts_to_each_execution() {
    let f = Fixture::new().await;
    std::fs::create_dir_all(f.root.join("workspace")).unwrap();
    std::fs::write(
        f.root.join("workspace/seed.txt"),
        "Synthetic initial workspace",
    )
    .unwrap();
    crate::research_files::capture(&f.state, "scan-example", &f.workspace_id).unwrap();
    // No process has started for the fixture's prepared generation.
    super::supervisor::reconcile_owned_attempt(
        &f.store,
        &f.store.worker_bootstrap(&f.attempt.id).unwrap().identity,
        true,
        WorkerDisposition::Failed,
        unix_ms(),
    )
    .unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    let app=axum::Router::new().route("/v1/chat/completions",axum::routing::post(move || {
        let counter=counter.clone(); async move {
            let index=counter.fetch_add(1,Ordering::SeqCst);
            let delta=if index.is_multiple_of(2) {
                json!({"tool_calls":[{"index":0,"id":"reused-provider-call","type":"function","function":{"name":"swarm_message_parent","arguments":json!({"message":format!("Synthetic result {}",index/2),"handoff":null}).to_string()}}]})
            } else {json!({"content":"Synthetic turn complete"})};
            let chunk=json!({"choices":[{"delta":delta}]});
            ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
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
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let host = LocalWorkerHost::start(f.state.clone()).await.unwrap();
    let files = crate::research_files::store(&f.state).unwrap();
    let mut directory = None;
    for index in 0..2 {
        f.store
            .local_message(
                "scan-example",
                &format!("resume-{index}"),
                &f.deputy,
                "Continue synthetic research",
            )
            .unwrap();
        let turn = f
            .store
            .local_claim("scan-example", &f.deputy, unix_ms(), 64)
            .unwrap()
            .unwrap();
        assert_eq!(
            host.run(&turn, None).await.unwrap(),
            WorkerReconciliation::TurnAccepted
        );
        assert!(f.store.worker_prepared(&turn.attempt_id).unwrap());
        assert!(
            files
                .environment("scan-example", &f.deputy)
                .unwrap()
                .checkpoint
                .is_some()
        );
        let path = files.prepare_agent("scan-example", &f.deputy).unwrap();
        assert_eq!(
            std::fs::read_to_string(path.join("seed.txt")).unwrap(),
            "Synthetic initial workspace"
        );
        if let Some(prior) = &directory {
            assert_eq!(prior, &path);
            assert_eq!(
                std::fs::read_to_string(path.join("retained.txt")).unwrap(),
                "Synthetic retained edit"
            );
        } else {
            std::fs::write(path.join("retained.txt"), "Synthetic retained edit").unwrap();
            directory = Some(path);
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    let local = f.store.local_state("scan-example").unwrap();
    for index in 0..2 {
        assert_eq!(
            local
                .inbox
                .iter()
                .filter(|m| m.message == format!("Synthetic result {index}"))
                .count(),
            1
        );
    }
    f.state.db.with_connection(|db| {
        let canonical:i64=db.query_row("SELECT count(*) FROM swarm_receipts WHERE json_extract(content,'$.kind')='canonical'",[],|r|r.get(0))?;
        assert_eq!(canonical,2);
        let catalogs:i64=db.query_row("SELECT count(*) FROM swarm_receipts WHERE json_extract(content,'$.operation.kind')='plugin_resources'",[],|r|r.get(0))?;
        assert_eq!(catalogs,2);
        Ok(())
    }).unwrap();
    assert!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .attempts
            .iter()
            .all(|a| a.state == ExecutionState::Stopped)
    );
    drop(host);
    drop(files);
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn local_scheduler_retains_ownership_until_filesystem_checkpoint_recovery() {
    let f = Fixture::new().await;
    std::fs::create_dir_all(f.root.join("workspace")).unwrap();
    crate::research_files::capture(&f.state, "scan-example", &f.workspace_id).unwrap();
    super::supervisor::reconcile_owned_attempt(
        &f.store,
        &f.store.worker_bootstrap(&f.attempt.id).unwrap().identity,
        true,
        WorkerDisposition::Failed,
        unix_ms(),
    )
    .unwrap();
    f.store
        .local_message(
            "scan-example",
            "resume-checkpoint",
            &f.deputy,
            "Continue synthetic research",
        )
        .unwrap();
    let turn = f
        .store
        .local_claim("scan-example", &f.deputy, unix_ms(), 64)
        .unwrap()
        .unwrap();
    let (server, calls) = inference_provider(&f, false).await;
    f.state.db.with_connection(|db| {db.execute_batch("CREATE TRIGGER fail_environment_checkpoint BEFORE INSERT ON swarm_file_checkpoints BEGIN SELECT RAISE(ABORT,'synthetic environment full'); END;")?;Ok(())}).unwrap();
    let host = LocalWorkerHost::start(f.state.clone()).await.unwrap();
    assert!(host.run(&turn, None).await.is_err());
    drop(host);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.state.worker_recovery.lock().unwrap().len(), 1);
    assert_eq!(
        f.state.db.session(&f.deputy).unwrap().unwrap().turns.len(),
        1
    );
    assert!(
        f.store
            .local_claim("scan-example", &f.deputy, unix_ms(), 64)
            .unwrap()
            .is_none()
    );
    assert_ne!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .attempts
            .iter()
            .find(|a| a.id == turn.attempt_id)
            .unwrap()
            .state,
        ExecutionState::Stopped
    );
    LocalWorkerHost::recover(&f.state, "scan-example").unwrap();
    assert_eq!(f.state.worker_recovery.lock().unwrap().len(), 1);
    f.state
        .db
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_environment_checkpoint;")?;
            Ok(())
        })
        .unwrap();
    LocalWorkerHost::recover(&f.state, "scan-example").unwrap();
    LocalWorkerHost::recover(&f.state, "scan-example").unwrap();
    assert!(f.state.worker_recovery.lock().unwrap().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.state.db.session(&f.deputy).unwrap().unwrap().turns.len(),
        2
    );
    assert_eq!(
        f.store
            .load("scan-example", true)
            .unwrap()
            .attempts
            .iter()
            .find(|a| a.id == turn.attempt_id)
            .unwrap()
            .state,
        ExecutionState::Stopped
    );
    assert!(
        crate::research_files::store(&f.state)
            .unwrap()
            .environment("scan-example", &f.deputy)
            .unwrap()
            .checkpoint
            .is_some()
    );
    server.abort();
    let _ = server.await;
    f.close().await;
}

#[tokio::test]
async fn local_scheduler_rejects_selected_destinations_and_keeps_corrupt_prepared_ownership() {
    let f = Fixture::new().await;
    let turn = f.store.worker_progress(&f.attempt.id).unwrap().turn;
    let host = LocalWorkerHost::start(f.state.clone()).await.unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    f.state.db.with_connection(|db| {db.execute("UPDATE swarm_worker_runtime SET context='corrupt synthetic checkpoint' WHERE attempt_id=?1",[&f.attempt.id])?;Ok(())}).unwrap();
    assert!(host.run(&turn, None).await.is_err());
    assert!(f.store.worker_prepared(&f.attempt.id).unwrap());
    assert!(f.store.worker_progress(&f.attempt.id).is_err());
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    f.state.db.with_connection(|db| {db.execute(r#"UPDATE swarm_scans SET document=json_set(document,'$.config.destinations',json('[{"host_id":"host-example","configuration_id":"container-example"}]')) WHERE id='scan-example'"#,[])?;Ok(())}).unwrap();
    let before = f.store.load("scan-example", true).unwrap();
    assert!(
        host.run(&turn, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("cannot fall back")
    );
    assert_eq!(f.store.load("scan-example", true).unwrap(), before);
    assert!(f.state.worker_recovery.lock().unwrap().is_empty());
    drop(host);
    f.close().await;
}

#[path = "peer_coordinator_tests.rs"]
mod peer_coordinators;

#[tokio::test]
async fn assigned_worker_publishes_patch_and_test_handoff_references_before_parent_delivery() {
    use beale_contracts::swarm_workflows::*;
    let f = Fixture::with_definition(
        true,
        beale_contracts::swarm_worker::WorkerShell::native(),
        false,
        "workspace-example",
        None,
        beale_swarms::OAI_948_SWARM.snapshot.clone(),
        "patch_generation",
    )
    .await;
    let workspace = f.root.join("worker-workspace");
    std::fs::create_dir(&workspace).unwrap();
    for (name, content) in [
        ("patch.txt", "Synthetic patch bytes"),
        ("check.txt", "Synthetic test bytes"),
    ] {
        std::fs::write(workspace.join(name), content).unwrap();
    }
    let snapshot = beale_swarms::OAI_948_SWARM.snapshot.clone();
    let route = snapshot
        .workflow()
        .unwrap()
        .routes
        .iter()
        .find(|r| {
            r.from == "patch_generation" && r.to == "validation" && r.payload == PayloadKind::Patch
        })
        .unwrap()
        .id
        .clone();
    let handoff = HandoffRequest {
        id: "patch-handoff-example".into(),
        from_agent: f.deputy.clone(),
        parent_id: f.leader.clone(),
        route_id: route,
        destination_profile: ProfileRef {
            id: "validation".into(),
            revision: 1,
        },
        task: task(),
        scope: vec!["Synthetic example.test fixture".into()],
        record_revisions: vec![],
        artifact_refs: vec![],
        observations: vec!["Synthetic patch candidate".into()],
        unresolved_questions: vec![],
        payload: HandoffPayload::Patch {
            patch_refs: vec!["file:patch.txt".into()],
            test_refs: vec!["file:check.txt".into()],
        },
    };
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    let app=axum::Router::new().route("/v1/chat/completions",axum::routing::post(move |axum::Json(body):axum::Json<Value>| {
        let counter=counter.clone(); let handoff=handoff.clone(); async move {
            let delta=match counter.fetch_add(1,Ordering::SeqCst) {
                0=>json!({"tool_calls":[{"index":0,"id":"patch-call-example","type":"function","function":{"name":"swarm_message_parent","arguments":json!({"message":"Check synthetic patch","handoff":handoff}).to_string()}}]}),
                1=>{
                    let reply=body["messages"].as_array().unwrap().iter().rev().find(|m|m["role"]=="tool").unwrap();
                    assert!(!reply["content"].to_string().contains("not acknowledged"),"{reply}");
                    json!({"content":"Synthetic patch submitted"})
                },
                _=>panic!("unexpected repeat provider dispatch")
            };
            let chunk=json!({"choices":[{"delta":delta}]});
            ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
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
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let launch = || WorkerLaunch {
        version: 1,
        identity: f.store.worker_bootstrap(&f.attempt.id).unwrap().identity,
        leader: f.url.clone(),
        bearer: f.credential.bearer().into(),
        workspace: workspace.clone(),
        protected_root: f.root.clone(),
        recovery: f.root.join("worker-recovery"),
    };
    tokio::time::timeout(Duration::from_secs(15), run_assigned(launch()))
        .await
        .unwrap()
        .unwrap();
    let saved = f.store.load("scan-example", true).unwrap();
    assert_eq!(saved.handoffs.len(), 1);
    let HandoffPayload::Patch {
        patch_refs,
        test_refs,
    } = &saved.handoffs[0].request.payload
    else {
        panic!("expected patch")
    };
    assert_ne!(patch_refs, test_refs);
    let files = crate::research_files::store(&f.state).unwrap();
    for (id, bytes) in [
        (&patch_refs[0], b"Synthetic patch bytes".as_slice()),
        (&test_refs[0], b"Synthetic test bytes".as_slice()),
    ] {
        assert!(!id.starts_with("file:"));
        let artifact = files.artifact(&f.workspace_id, id).unwrap();
        assert_eq!(artifact.producer, f.deputy);
        assert_eq!(artifact.attempt, f.attempt.id);
        assert_eq!(
            files.read_blob(&artifact.sha256, artifact.size).unwrap(),
            bytes
        );
    }
    run_assigned(launch()).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        f.store.load("scan-example", true).unwrap().handoffs,
        saved.handoffs
    );
    server.abort();
    let _ = server.await;
    drop(files);
    f.close().await;
}

#[tokio::test]
async fn worker_reconciliation_preserves_operator_transcript_and_model_input_after_reopen() {
    use beale_contracts::swarm_worker::WorkerShell;
    let f = Fixture::new().await;
    f.store
        .reconcile_worker(&f.attempt.id, true, WorkerDisposition::Failed, unix_ms())
        .unwrap();
    let leader = f
        .store
        .local_claim("scan-example", &f.leader, unix_ms(), 64)
        .unwrap()
        .unwrap();
    f.store
        .local_dispatch(
            &leader,
            "parent-followup",
            SwarmCommand::MessageChild(beale_contracts::swarm_local::MessageChild {
                child_id: f.deputy.clone(),
                message: "Internal synthetic parent input".into(),
            }),
            Admission::Ready,
        )
        .unwrap();
    let operator = "  Retry synthetic worker — preserve exact text.\n";
    f.store
        .local_message("scan-example", "operator-followup", &f.deputy, operator)
        .unwrap();
    let turn = f
        .store
        .local_claim("scan-example", &f.deputy, unix_ms(), 64)
        .unwrap()
        .unwrap();
    assert_eq!(turn.operator_text.as_deref(), Some(operator));
    assert!(turn.prompt.contains("Internal synthetic parent input"));
    prepare_worker(
        &f.state.db,
        &turn,
        WorkerShell::native(),
        &registration(),
        unix_ms(),
    )
    .unwrap();
    f.store
        .reconcile_worker(&turn.attempt_id, true, WorkerDisposition::Failed, unix_ms())
        .unwrap();
    let db = beale_storage::HostDatabase::open(&f.root.join("host")).unwrap();
    let (session, turns) = crate::load_persisted(&db, Uuid::parse_str(&f.deputy).unwrap()).unwrap();
    let response = crate::detail(&db, &session, &turns).unwrap();
    let users: Vec<_> = response
        .messages
        .iter()
        .filter(|m| m.role == beale_contracts::v1::SessionRole::User as i32)
        .map(|m| m.text.as_str())
        .collect();
    assert_eq!(users, vec![operator]);
    assert_eq!(
        db.session_model_prompts(&f.deputy).unwrap()[&2],
        turn.prompt
    );
    f.store
        .local_message(
            "scan-example",
            "resume-followup",
            &f.deputy,
            "Continue synthetic work",
        )
        .unwrap();
    let resumed = f
        .store
        .local_claim("scan-example", &f.deputy, unix_ms(), 64)
        .unwrap()
        .unwrap();
    let (bootstrap, _) = prepare_worker(
        &db,
        &resumed,
        WorkerShell::native(),
        &registration(),
        unix_ms(),
    )
    .unwrap();
    assert!(
        bootstrap
            .input
            .iter()
            .any(|item| item["role"] == "user" && item["content"] == turn.prompt)
    );
    drop(db);
    f.close().await;
}
