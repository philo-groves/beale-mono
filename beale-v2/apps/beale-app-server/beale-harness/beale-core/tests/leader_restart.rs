#![cfg(feature = "swarms")]

use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
    routing::post,
};
use beale_contracts::{v1::*, *};
use beale_providers::{CredentialStore, CustomProvider, ModelEntry};
use beale_storage::{HostDatabase, swarm_state::SwarmStore};
use prost::Message;
use serde_json::{Value, json};
use std::{
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tower::ServiceExt;

fn start(root: &Path, address: &str) -> tokio::process::Child {
    beale_tools::process::contained_command(env!("CARGO_BIN_EXE_beale-core"))
        .arg("serve")
        .env("BEALE_V2_HOME", root)
        .env("BEALE_V2_APP_SERVER_ADDR", address)
        .env("HOME", root)
        .env_remove("BEALE_FLEET_LEADER_ORIGIN")
        .env_remove("BEALE_CONTAINER_RUNTIME")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}
async fn ready(client: &reqwest::Client, origin: &str, child: &mut tokio::process::Child) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if client
            .get(format!("{origin}{HEALTH_PATH}"))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            return;
        }
        assert!(child.try_wait().unwrap().is_none(), "fixture host exited");
        assert!(
            tokio::time::Instant::now() < deadline,
            "host readiness timed out"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn killed_leader_process_recovers_through_normal_startup_without_provider_redispatch() {
    process_restart(None).await;
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "requires a built Desktop executable and interactive Windows session"]
async fn desktop_and_leader_process_restart_preserves_recovered_session() {
    let desktop = std::env::var_os("BEALE_TEST_DESKTOP")
        .expect("set BEALE_TEST_DESKTOP to the built Desktop executable");
    process_restart(Some(std::path::PathBuf::from(desktop))).await;
}

async fn process_restart(desktop: Option<std::path::PathBuf>) {
    let root = std::env::temp_dir().join(format!(
        "beale-process-restart-example-{}",
        uuid::Uuid::new_v4()
    ));
    let host = root.join("host");
    let workspaces = beale_workspaces::WorkspaceStore::at(host.clone()).unwrap();
    let credentials = CredentialStore::at(host.clone());
    let token = credentials.control_token().unwrap();
    let response = beale_workspaces::router(token.clone(), workspaces.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(WORKSPACES_PATH)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", PROTOBUF_CONTENT_TYPE)
                .body(Body::from(
                    CreateWorkspaceRequest {
                        name: "Example restart workspace".into(),
                        directory: root.join("workspace").to_string_lossy().into_owned(),
                        rules: vec!["Only synthetic example.test fixtures are authorized.".into()],
                        ..Default::default()
                    }
                    .encode_to_vec(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let workspace =
        WorkspaceSummary::decode(to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    let db = HostDatabase::open(&host).unwrap();
    beale_sessions::features::migrate(&db).unwrap();
    db.set_feature_enabled("swarms", true).unwrap();
    let store = SwarmStore::open(db.clone()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let app=Router::new().route("/v1/chat/completions",post(move |axum::Json(body):axum::Json<Value>| {
        let counter=counter.clone(); async move {
            let text=body.to_string();
            let delta=if text.contains("host-side reviewer") {
                json!({"content":"{\"decision\":\"approved\",\"reason\":\"Synthetic managerial coordination\"}"})
            } else if counter.fetch_add(1,Ordering::SeqCst)==0 {
                json!({"tool_calls":[{"index":0,"id":"spawn-example","type":"function","function":{"name":"swarm_spawn_subagent","arguments":json!({"profile":{"id":"research_deputy","revision":1},"task":{"question":"Wait for synthetic follow-up","completion_criteria":["Return synthetic evidence"],"evidence_refs":[]},"independent":false}).to_string()}}]})
            } else {
                tokio::time::sleep(Duration::from_secs(120)).await;
                json!({"content":"Synthetic delayed response"})
            };
            let chunk=json!({"choices":[{"delta":delta}]});
            ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
    let provider_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    credentials
        .save_custom_provider(CustomProvider {
            id: "custom-example".into(),
            name: "Example".into(),
            base_url: format!("http://{}/v1", provider_listener.local_addr().unwrap()),
            models: vec![ModelEntry {
                id: "example".into(),
                name: "Example".into(),
            }],
            lead_model: "example".into(),
            small_model: "example".into(),
            reasoning: false,
        })
        .unwrap();
    let server = tokio::spawn(async move { axum::serve(provider_listener, app).await.unwrap() });
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap().to_string();
    drop(reservation);
    let origin = format!("http://{address}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let launch_desktop = || {
        desktop.as_ref().map(|path| {
            beale_tools::process::contained_command(path)
                .env("BEALE_V2_HOME", &host)
                .env("BEALE_V2_APP_SERVER_ADDR", &address)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .unwrap()
        })
    };
    let mut child = start(&host, &address);
    ready(&client, &origin, &mut child).await;
    let mut desktop_process = launch_desktop();
    if let Some(app) = &mut desktop_process {
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(
            app.try_wait().unwrap().is_none(),
            "Desktop exited during startup"
        );
    }
    let submit = SubmitSessionTurnRequest {
        workspace_id: workspace.id,
        prompt: "Coordinate synthetic restart fixture".into(),
        swarm_id: "basic".into(),
        provider: ProviderId::Custom as i32,
        method: AuthMethod::ApiKey as i32,
        model: "custom-example::example".into(),
        ..Default::default()
    };
    let stream = client
        .post(format!("{origin}{SESSION_TURN_STREAM_PATH}"))
        .bearer_auth(&token)
        .header("content-type", PROTOBUF_CONTENT_TYPE)
        .body(submit.encode_to_vec())
        .send()
        .await
        .unwrap();
    assert!(stream.status().is_success());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    let scan = loop {
        let scans: Vec<String> = db
            .with_connection(|db| {
                let mut query = db.prepare("SELECT id FROM swarm_scans")?;
                Ok(query
                    .query_map([], |r| r.get(0))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .unwrap();
        if let Some(scan) = scans.first()
            && let Ok(saved) = store.load(scan, true)
            && saved.agents.len() == 2
            && calls.load(Ordering::SeqCst) >= 2
        {
            break scan.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "subordinate was not scheduled"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    // Drop the client transport, then terminate the actual host while its leader is running.
    drop(stream);
    if let Some(app) = &mut desktop_process {
        app.kill().await.unwrap();
        app.wait().await.unwrap();
    }
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    let dispatched = calls.load(Ordering::SeqCst);
    let mut restarted = start(&host, &address);
    ready(&client, &origin, &mut restarted).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let saved = store.load(&scan, true).unwrap();
        if saved
            .events
            .iter()
            .any(|e| e.kind == "coordinator-interrupted")
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "normal maintenance failed to recover dead leader"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut desktop_restarted = launch_desktop();
    if let Some(app) = &mut desktop_restarted {
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(
            app.try_wait().unwrap().is_none(),
            "Desktop exited on reconnect"
        );
        let response = client
            .get(format!(
                "{origin}{}",
                SESSION_DETAIL_PATH.replace("{id}", &scan)
            ))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let detail = SessionDetail::decode(response.bytes().await.unwrap()).unwrap();
        assert_eq!(detail.summary.unwrap().id, scan);
        app.kill().await.unwrap();
        app.wait().await.unwrap();
    }
    let saved = store.load(&scan, true).unwrap();
    assert_eq!(
        saved
            .events
            .iter()
            .filter(|e| e.kind == "coordinator-interrupted")
            .count(),
        1
    );
    assert!(
        store
            .local_state(&scan)
            .unwrap()
            .controls
            .iter()
            .all(|c| c.stop_requested)
    );
    assert!(
        db.session(&scan)
            .unwrap()
            .unwrap()
            .turns
            .iter()
            .any(|t| t.answer.starts_with("Host recovery:"))
    );
    assert_eq!(calls.load(Ordering::SeqCst), dispatched);
    restarted.kill().await.unwrap();
    restarted.wait().await.unwrap();
    let mut again = start(&host, &address);
    ready(&client, &origin, &mut again).await;
    assert_eq!(
        store
            .load(&scan, true)
            .unwrap()
            .events
            .iter()
            .filter(|e| e.kind == "coordinator-interrupted")
            .count(),
        1
    );
    assert_eq!(calls.load(Ordering::SeqCst), dispatched);
    again.kill().await.unwrap();
    again.wait().await.unwrap();
    server.abort();
    drop(store);
    drop(db);
    drop(workspaces);
    let _ = std::fs::remove_dir_all(root);
}
