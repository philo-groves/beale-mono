use anyhow::{Context, Result, bail, ensure};
use axum::{
    Router,
    http::{HeaderMap, HeaderValue, StatusCode},
    routing::get,
};
use beale_contracts::{
    DEFAULT_LOCAL_ADDR, HEALTH_PATH, PROTOBUF_CONTENT_TYPE, PROTOCOL_MAJOR, PROTOCOL_MINOR,
    SERVICE_NAME, v1::HealthResponse,
};
use beale_providers::{
    AuthMode, CredentialStore, InferenceRequest, Provider, ProviderClient, TextMessage,
};
use prost::Message;
use std::{io::Read, net::SocketAddr};
use tokio::net::TcpListener;

mod auth;
#[cfg(all(feature = "swarms", target_os = "linux"))]
mod container;
mod features;
#[cfg(windows)]
mod tray;

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(all(feature = "swarms", target_os = "linux"))]
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("worker-supervisor")) {
        ensure!(
            std::env::args_os().count() == 3,
            "supervisor directory required"
        );
        let directory = std::path::PathBuf::from(
            std::env::args_os()
                .nth(2)
                .context("missing supervisor directory")?,
        );
        return beale_sessions::distributed::linux_worker_supervisor(&directory);
    }
    #[cfg(all(feature = "swarms", target_os = "linux"))]
    match std::env::args().nth(1).as_deref() {
        Some("container-idle") => return container::idle().await,
        Some("container-probe") => return container::probe().await,
        Some("container-worker") => beale_tools::process::establish_container_worker_group()?,
        _ => (),
    }
    #[cfg(feature = "swarms")]
    if matches!(std::env::args().nth(1).as_deref(), Some("worker"))
        || cfg!(target_os = "linux")
            && matches!(
                std::env::args().nth(1).as_deref(),
                Some("container-worker" | "container-prepare")
            )
    {
        ensure!(
            std::env::args().count() == 2,
            "worker configuration must arrive on stdin"
        );
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(beale_contracts::swarms::MAX_DOCUMENT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= beale_contracts::swarms::MAX_DOCUMENT_BYTES,
            "worker configuration too large"
        );
        if std::env::args().nth(1).as_deref() == Some("container-prepare") {
            let input = serde_json::from_slice::<
                beale_sessions::distributed::docker::ContainerPreparation,
            >(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid container preparation"))?;
            use beale_contracts::swarm_container::{PreparationFailure, PreparationReport};
            let failure = beale_sessions::distributed::docker::prepare_container(input)
                .await
                .err()
                .map(|e| {
                    e.downcast_ref::<PreparationFailure>()
                        .copied()
                        .unwrap_or(PreparationFailure::Runtime)
                });
            println!(
                "{}",
                serde_json::to_string(&PreparationReport {
                    version: 1,
                    failure
                })?
            );
            return Ok(());
        }
        let launch = serde_json::from_slice::<beale_sessions::distributed::WorkerLaunch>(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid worker configuration"))?;
        return beale_sessions::distributed::worker_entry(launch)
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "assigned worker stopped; retained recovery state requires reconciliation"
                )
            });
    }
    let path = beale_host_log::init("app-server")?;
    beale_host_log::info!("app-server starting; log file: {}", path.display());
    let result = run().await;
    if let Err(error) = &result {
        beale_host_log::error!("app-server exited with an error: {error:#}");
    }
    result
}

async fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None | Some("serve") => serve().await,
        Some("login-xai") => {
            let client = ProviderClient::new()?;
            beale_providers::login_xai(client.http(), &CredentialStore::new()?).await?;
            println!("xAI sign-in saved on this host.");
            Ok(())
        }
        Some("infer") => {
            let provider = match args.next().as_deref() {
                Some("openai") => Provider::OpenAi,
                Some("xai") => Provider::Xai,
                _ => bail!("provider must be openai or xai"),
            };
            let auth = match args.next().as_deref() {
                Some("api-key") => AuthMode::ApiKey,
                Some("subscription") => AuthMode::Subscription,
                _ => bail!("auth mode must be api-key or subscription"),
            };
            let model = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("model is required"))?;
            if args.next().is_some() {
                bail!("unexpected argument");
            }
            let mut prompt = String::new();
            std::io::stdin()
                .take(1_048_577)
                .read_to_string(&mut prompt)?;
            let client = ProviderClient::new()?;
            let response = client
                .infer(
                    provider,
                    auth,
                    &InferenceRequest {
                        model,
                        messages: vec![TextMessage {
                            role: beale_providers::TextRole::User,
                            text: prompt,
                        }],
                    },
                    &CredentialStore::new()?,
                )
                .await?;
            println!("{response}");
            Ok(())
        }
        _ => bail!(
            "usage: beale-core [serve | login-xai | infer <openai|xai> <api-key|subscription> <model>] (use codex login for OpenAI subscription)"
        ),
    }
}

async fn serve() -> Result<()> {
    // Client authentication and Tailscale binding are separate contract work.
    // This first host slice only exposes a bounded local health response.
    let configured = match std::env::var("BEALE_V2_APP_SERVER_ADDR") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => DEFAULT_LOCAL_ADDR.to_owned(),
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!("BEALE_V2_APP_SERVER_ADDR must contain a valid address")
        }
    };
    let addr = local_listen_addr(&configured)?;
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to listen on {addr}"))?;
    println!(
        "Beale app-server listening on http://{}",
        listener.local_addr()?
    );
    beale_host_log::info!("app-server listening on {}", listener.local_addr()?);
    let store = CredentialStore::new()?;
    let token = store.control_token()?;
    #[cfg(windows)]
    let tray_token = token.clone();
    let workspaces = beale_workspaces::WorkspaceStore::new()?;
    let db = beale_storage::HostDatabase::open(workspaces.root())?;
    let execution_db = db.clone();
    let execution_data_root = workspaces.root().to_path_buf();
    #[cfg(feature = "swarms")]
    let (execution_backend, docker_recovery, allocator): (
        std::sync::Arc<dyn beale_fleet::worker_host::HostExecutionBackend>,
        tokio::task::JoinHandle<()>,
        beale_fleet::allocation::Allocator,
    ) = {
        let environments = beale_fleet::docker::DockerEnvironments::new(
            beale_fleet::docker::Docker::new(
                std::env::var_os("BEALE_DOCKER_CLI")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| "docker".into()),
            ),
            beale_fleet::docker::DockerStore::open(db.clone())?,
            execution_data_root.clone(),
            std::env::var_os("BEALE_CONTAINER_RUNTIME").map(std::path::PathBuf::from),
        )?;
        let recovery = environments.start_recovery();
        let allocator = beale_fleet::allocation::Allocator::new(
            db.clone(),
            environments,
            std::sync::Arc::new(beale_fleet::allocation::SystemCapacity {
                root: execution_data_root.clone(),
            }),
        )?;
        (
            std::sync::Arc::new(
                beale_sessions::distributed::peer_host::AssignedWorkerBackend::new(
                    execution_data_root.clone(),
                    std::sync::Arc::new(
                        beale_sessions::distributed::docker::AllocatedDockerBackend(
                            allocator.clone(),
                        ),
                    ),
                ),
            ),
            recovery,
            allocator,
        )
    };
    #[cfg(not(feature = "swarms"))]
    let execution_backend: std::sync::Arc<dyn beale_fleet::worker_host::HostExecutionBackend> =
        std::sync::Arc::new(beale_fleet::worker_host::UnavailableBackend);
    beale_sessions::features::migrate(&db)?;
    beale_fleet::FleetCoordinationStore::open(db.clone())?;
    beale_sessions::features::migrate_features(&db, &[&beale_tracking::FEATURE])?;
    let plugin_data_root = workspaces.root().to_path_buf();
    let plugins = beale_extender::PluginRuntime::new(db.clone(), plugin_data_root)?;
    let sessions =
        beale_sessions::SessionRuntime::new(token.clone(), workspaces.clone(), plugins.clone())?;
    #[cfg(feature = "swarms")]
    let sessions = {
        let local_origin = format!("http://{}", listener.local_addr()?);
        beale_fleet::FleetCoordinationStore::open(db.clone())?
            .save_local_endpoint(&local_origin, &token)?;
        let leader_origin = std::env::var("BEALE_FLEET_LEADER_ORIGIN").unwrap_or(local_origin);
        sessions.with_fleet_origin(leader_origin)?
    };
    let accounting_maintenance = beale_providers::pacing::start_maintenance(db.clone())?;
    let app = Router::new()
        .merge(beale_providers::pacing::http::router(
            token.clone(),
            db.clone(),
        ))
        .route(HEALTH_PATH, get(health))
        .merge(beale_workspaces::router(token.clone(), workspaces.clone()))
        .merge(sessions.router())
        .merge(features::router(token.clone(), db.clone()));
    #[cfg(feature = "swarms")]
    let app = app.merge(beale_fleet::allocation::http::router(
        token.clone(),
        allocator.clone(),
    ));
    #[cfg(feature = "findings")]
    let app = app.merge(beale_findings::router(token.clone(), db.clone()));
    #[cfg(feature = "memories")]
    let app = app.merge(beale_memories::router(
        token.clone(),
        db.clone(),
        workspaces.root().to_path_buf(),
    ));
    #[cfg(feature = "runbooks")]
    let app = app.merge(beale_runbooks::router(token.clone(), db.clone()));
    let app = app.merge(beale_tracking::router(token.clone(), db.clone()));
    let app = app.merge(beale_fleet::router_with_backend(
        token.clone(),
        db.clone(),
        workspaces.root().to_path_buf(),
        execution_backend.clone(),
    ));
    let app = app
        .merge(beale_safety::router(token.clone(), db.clone()))
        .merge(beale_extender::router_with_runtime(
            token.clone(),
            plugins.clone(),
        )?)
        .merge(auth::router(token, store, db));
    #[cfg(windows)]
    let (tray_exit_tx, tray_exit_rx) = tokio::sync::oneshot::channel();
    #[cfg(windows)]
    let _tray = tray::TrayHandle::start(addr, tray_token, tray_exit_tx)?;
    #[cfg(feature = "memories")]
    let memory_worker = beale_sessions::start_memory_worker(workspaces.clone(), plugins.clone())?;
    let scheduler = sessions.start_automation_scheduler();
    #[cfg(feature = "swarms")]
    let allocation_maintenance = allocator.start_maintenance();
    let execution_recovery = beale_fleet::worker_host::start_recovery(
        execution_db,
        execution_data_root,
        execution_backend,
    );
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            #[cfg(windows)]
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = tray_exit_rx => {},
            }
            #[cfg(not(windows))]
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    accounting_maintenance.abort();
    scheduler.abort();
    execution_recovery.abort();
    #[cfg(feature = "swarms")]
    docker_recovery.abort();
    #[cfg(feature = "swarms")]
    allocation_maintenance.abort();
    #[cfg(feature = "memories")]
    memory_worker.abort();
    result?;
    beale_host_log::info!("app-server stopped");
    Ok(())
}

fn local_listen_addr(value: &str) -> Result<SocketAddr> {
    let addr: SocketAddr = value
        .parse()
        .with_context(|| format!("invalid BEALE_V2_APP_SERVER_ADDR: {value}"))?;
    ensure!(
        addr.ip().is_loopback() && addr.port() != 0,
        "BEALE_V2_APP_SERVER_ADDR must be a loopback address with a nonzero port"
    );
    Ok(addr)
}

async fn health() -> (StatusCode, HeaderMap, Vec<u8>) {
    let body = HealthResponse {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        service: SERVICE_NAME.to_owned(),
    }
    .encode_to_vec();
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static(PROTOBUF_CONTENT_TYPE),
    );
    (StatusCode::OK, headers, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_uses_versioned_contract() {
        let (_, headers, body) = health().await;
        assert_eq!(headers["content-type"], PROTOBUF_CONTENT_TYPE);
        let message = HealthResponse::decode(body.as_slice()).unwrap();
        assert_eq!(message.protocol_major, PROTOCOL_MAJOR);
        assert_eq!(message.service, SERVICE_NAME);
    }

    #[test]
    fn listen_address_stays_on_loopback() {
        assert!(local_listen_addr(DEFAULT_LOCAL_ADDR).is_ok());
        assert!(local_listen_addr("[::1]:43831").is_ok());
        assert!(local_listen_addr("0.0.0.0:43831").is_err());
        assert!(local_listen_addr("127.0.0.1:0").is_err());
    }
}
