use super::DockerStore;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use beale_contracts::swarm_container::*;
use beale_storage::HostDatabase;

#[derive(Clone)]
struct ConfigurationState {
    token: String,
    db: HostDatabase,
}
pub fn router(token: String, db: HostDatabase) -> Router {
    Router::new()
        .route(CONFIGURATIONS_PATH, get(list).post(save))
        .route(
            beale_contracts::swarm_ui::PEER_REGISTRATION_PATH,
            axum::routing::post(register_peer),
        )
        .layer(DefaultBodyLimit::max(65_536))
        .with_state(ConfigurationState { token, db })
}
async fn register_peer(
    State(state): State<ConfigurationState>,
    headers: HeaderMap,
    Json(peer): Json<beale_contracts::swarm_ui::PeerRegistration>,
) -> Result<Json<bool>, StatusCode> {
    store(&state, &headers)?;
    beale_storage::fleet_coordination::FleetCoordinationStore::open(state.db)
        .and_then(|store| {
            store.save_peer(&beale_storage::fleet_coordination::FleetPeer {
                id: peer.host_id,
                endpoint: peer.endpoint,
                control_token: peer.control_token,
                enabled: peer.enabled,
                protocol_major: peer.protocol_major,
            })
        })
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(Json(true))
}
fn store(state: &ConfigurationState, headers: &HeaderMap) -> Result<DockerStore, StatusCode> {
    if headers.get("authorization").and_then(|v| v.to_str().ok())
        != Some(format!("Bearer {}", state.token).as_str())
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    DockerStore::open(state.db.clone()).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
async fn list(
    State(state): State<ConfigurationState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ConfigurationReadiness>>, StatusCode> {
    Ok(Json(
        store(&state, &headers)?
            .configurations()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    ))
}
async fn save(
    State(state): State<ConfigurationState>,
    headers: HeaderMap,
    Json(config): Json<ImageConfiguration>,
) -> Result<Json<ConfigurationReadiness>, StatusCode> {
    let store = store(&state, &headers)?;
    config.validate().map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(Json(
        store
            .save_configuration(config)
            .map_err(|_| StatusCode::CONFLICT)?,
    ))
}
