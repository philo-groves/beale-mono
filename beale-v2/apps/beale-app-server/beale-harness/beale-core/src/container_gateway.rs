//! Worker route for containers on a native Linux Docker engine.
//!
//! Inside a container, loopback is the container itself, so workers address
//! the leader as `CONTAINER_HOST_ALIAS` on the leader's port. Docker Desktop
//! forwards that alias to the host's loopback listener. A native engine maps
//! it to its bridge gateway when the container is created, so the host also
//! listens there, serving only the worker route. That route accepts only
//! per-attempt worker credentials; the host control API stays on loopback.
//! This is host-local routing, not an isolation boundary.
use axum::Router;
use beale_fleet::docker::{Docker, HostAccess};
use std::{net::SocketAddr, time::Duration};

const FIRST_RETRY: Duration = Duration::from_secs(5);
const MAX_RETRY: Duration = Duration::from_secs(300);

/// Serves `router` on the bridge gateway at `port` for as long as the engine
/// is native. Docker may start after the host, so detection is retried with
/// backoff. Returns once the engine is identified as Docker Desktop.
pub async fn serve(docker: Docker, port: u16, router: Router) {
    let mut retry = FIRST_RETRY;
    let mut reported = false;
    loop {
        match docker.host_access().await {
            Ok(HostAccess::Desktop) => return,
            Ok(HostAccess::Bridge(gateway)) => {
                let addr = SocketAddr::from((gateway, port));
                match tokio::net::TcpListener::bind(addr).await {
                    Ok(listener) => {
                        beale_host_log::info!("container worker route listening on {addr}");
                        retry = FIRST_RETRY;
                        if let Err(error) = axum::serve(listener, router.clone()).await {
                            beale_host_log::error!("container worker route stopped: {error}");
                        }
                    }
                    Err(error) => {
                        beale_host_log::error!(
                            "container worker route cannot listen on {addr}: {error}"
                        );
                    }
                }
            }
            Err(error) => {
                // Docker is optional; report its absence once, then retry quietly.
                if !reported {
                    beale_host_log::info!(
                        "Docker is unavailable; container worker route is not serving: {error:#}"
                    );
                    reported = true;
                }
            }
        }
        tokio::time::sleep(retry).await;
        retry = (retry * 2).min(MAX_RETRY);
    }
}
