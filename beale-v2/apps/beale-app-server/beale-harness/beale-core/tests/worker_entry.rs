#![cfg(feature = "swarms")]

use axum::{Json, Router, http::HeaderMap, routing::post};
use beale_contracts::{swarm_worker::*, swarms::*};
use beale_sessions::distributed::WorkerLaunch;
use beale_tools::process::{ProcessTree, contained_command};
use serde_json::json;
use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::AsyncWriteExt;

#[cfg(target_os = "linux")]
#[tokio::test]
async fn linux_supervisor_binary_runs_before_host_initialization_and_preserves_proof() {
    use beale_tools::linux_worker::LinuxWorker;
    use std::path::Path;
    let root = std::env::temp_dir().join(format!(
        "beale-supervisor-entry-example-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&root).unwrap();
    let unavailable = root.join("unavailable-host");
    std::fs::write(&unavailable, b"Synthetic regular file").unwrap();
    let key = uuid::Uuid::new_v4();
    let supervisor = LinuxWorker::prepare(
        &root.join("owners"),
        key,
        Path::new("/bin/sh"),
        &[
            "-c".into(),
            "cat >/dev/null; test -z \"$OPENAI_API_KEY\" || exit 9; echo ready > ready; sleep 30"
                .into(),
        ],
        &root,
    )
    .unwrap();
    let mut child = contained_command(env!("CARGO_BIN_EXE_beale-core"))
        .arg("worker-supervisor")
        .arg(supervisor.directory())
        .kill_on_drop(false)
        .env("BEALE_V2_HOME", &unavailable)
        .env("HOME", &root)
        .env("OPENAI_API_KEY", "synthetic-guest-key")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"Synthetic startup")
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !root.join("ready").exists() {
        assert!(child.try_wait().unwrap().is_none());
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    supervisor.request_stop().unwrap();
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    assert!(
        LinuxWorker::open(&root.join("owners"), key)
            .unwrap()
            .recover_terminated()
            .unwrap()
    );
    assert!(unavailable.is_file());
    assert!(!root.join(".beale").exists());
    std::fs::remove_dir_all(root).unwrap();
}

/// Exercise the actual executable before host initialization. A deliberately
/// unusable guest host root must not prevent assigned inference or execution.
#[tokio::test]
async fn worker_binary_uses_leader_inference_and_private_spool_without_guest_host_services() {
    let root = std::env::temp_dir().join(format!(
        "beale-worker-entry-example-{}",
        uuid::Uuid::new_v4()
    ));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let guest_host = root.join("unavailable-guest-host");
    std::fs::write(
        &guest_host,
        "Synthetic regular file, not a host database directory",
    )
    .unwrap();
    let identity = ExecutionIdentity {
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: "worker-example".into(),
        attempt_id: "attempt-example".into(),
        generation: 1,
    };
    let bearer = "c".repeat(64);
    let steps = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicBool::new(false));
    let gateway_identity = identity.clone();
    let gateway_bearer = bearer.clone();
    let gateway_steps = steps.clone();
    let gateway_finished = finished.clone();
    let app = Router::new().route(WORKER_PATH, post(move |headers: HeaderMap, Json(request): Json<WorkerRequest>| {
        let identity = gateway_identity.clone();
        let bearer = gateway_bearer.clone();
        let steps = gateway_steps.clone();
        let finished = gateway_finished.clone();
        async move {
            assert_eq!(headers["authorization"], format!("Bearer {bearer}"));
            assert!(identity.matches(&request));
            request.validate().unwrap();
            if matches!(request.operation, WorkerOperation::Heartbeat {}) {
                return Json(WorkerReply::Lease { grant: LeaseGrant { identity, request_id: request.operation_id, valid_for_ms: 30_000 } });
            }
            let result = match &request.operation {
                WorkerOperation::Bootstrap {} => serde_json::to_value(WorkerBootstrap { identity, shell: WorkerShell::native(), route_id: "route-example".into(), message_format: MessageFormat::Chat, enabled_tools: vec![beale_tools::COMMAND_TOOL_NAME.into()], input: vec![json!({"role":"user","content":"Create a synthetic candidate."})] }).unwrap(),
                WorkerOperation::Infer { route_id, messages } => {
                    assert_eq!(route_id, "route-example");
                    let first = steps.fetch_add(1, Ordering::SeqCst) == 0;
                    let command = if cfg!(windows) {
                        "if ($env:OPENAI_API_KEY) { exit 9 }; Set-Content -LiteralPath candidate.txt -Value 'Synthetic candidate' -NoNewline"
                    } else {
                        "test -z \"$OPENAI_API_KEY\" && printf 'Synthetic candidate' > candidate.txt"
                    };
                    if !first {
                        assert!(messages.as_array().unwrap().iter().any(|m| m["tool_call_id"] == "command-example"));
                    }
                    serde_json::to_value(InferenceResult::Completed { text: if first { String::new() } else { "Synthetic completion".into() }, calls: if first { vec![InferenceCall { id: "command-example".into(), name: beale_tools::COMMAND_TOOL_NAME.into(), arguments: json!({"command":command,"timeout_seconds":5}).to_string() }] } else { vec![] }, reasoning_summaries: vec![], continuation: vec![], usage: None, response_id: None }).unwrap()
                }
                WorkerOperation::Review { call } => { assert_eq!(call.name, beale_tools::COMMAND_TOOL_NAME); json!({"approved":true}) }
                WorkerOperation::Checkpoint { revision, .. } => json!({"revision":revision}),
                WorkerOperation::Event { payload, .. } => { assert_eq!(payload["kind"], "tool_outcome"); assert_eq!(payload["result"]["is_error"], false); json!({"sequence":1}) }
                WorkerOperation::Finish { answer, .. } => { assert_eq!(answer, "Synthetic completion"); finished.store(true, Ordering::SeqCst); json!({"recorded":true,"termination_confirmed":false}) }
                _ => panic!("unexpected worker operation"),
            };
            Json(WorkerReply::Acknowledged { acknowledgement: MutationAcknowledgement { version: VERSION, operation_id: request.operation_id, result } })
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let leader = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let launch = WorkerLaunch {
        version: 1,
        identity,
        leader,
        bearer,
        workspace: workspace.clone(),
        protected_root: root.clone(),
        recovery: root.join("recovery"),
    };
    let mut child = contained_command(env!("CARGO_BIN_EXE_beale-core"))
        .arg("worker")
        .current_dir(&workspace)
        .env("BEALE_V2_HOME", &guest_host)
        .env("HOME", &root)
        .env("USERPROFILE", &root)
        .env("OPENAI_API_KEY", "synthetic-guest-key")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut tree = ProcessTree::attach_required(&child).unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(&serde_json::to_vec(&launch).unwrap())
        .await
        .unwrap();
    stdin.flush().await.unwrap();
    stdin.shutdown().await.unwrap();
    drop(stdin);
    // Unix deliberately terminates the owned group on completion, so process
    // exit status is not a substitute for the durable candidate finish record.
    tokio::time::timeout(Duration::from_secs(15), child.wait())
        .await
        .unwrap()
        .unwrap();
    tree.request_termination().unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !tree.confirm_terminated().unwrap() {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(finished.load(Ordering::SeqCst));
    assert_eq!(steps.load(Ordering::SeqCst), 2);
    assert_eq!(
        std::fs::read_to_string(workspace.join("candidate.txt")).unwrap(),
        "Synthetic candidate"
    );
    assert!(root.join("recovery/recovery.sqlite").exists());
    assert!(guest_host.is_file());
    assert!(!root.join(".beale").exists());
    server.abort();
    let _ = server.await;
    drop(child);
    drop(tree);
    std::fs::remove_dir_all(root).unwrap();
}
