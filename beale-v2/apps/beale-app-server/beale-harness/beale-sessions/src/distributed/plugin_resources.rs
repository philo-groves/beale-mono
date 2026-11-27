//! Resource preparation uses host configuration; reads use the accepted snapshot.
use super::gateway::unavailable;
use crate::{SessionState, tool_runtime};
use axum::http::StatusCode;
use beale_contracts::{
    swarm_plugins::ResourceRequest,
    swarms::{MutationAcknowledgement, WorkerOperation, WorkerRequest},
};
use beale_storage::swarm_state::{PluginResourceInput, PreparedPluginBundle, SwarmStore};

pub(super) async fn dispatch(
    state: &SessionState,
    store: &SwarmStore,
    bearer: &str,
    r: &WorkerRequest,
) -> Result<MutationAcknowledgement, StatusCode> {
    let WorkerOperation::PluginResources { request } = &r.operation else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let input = match request {
        ResourceRequest::Catalog {} => match state.plugins.available(&r.workspace_id) {
            Ok(plugins) => {
                let mut names: Vec<_> = plugins.into_iter().map(|p| p.name).collect();
                names.sort();
                if names.len() > 128 {
                    PluginResourceInput::Unavailable(
                        "Plugin catalog exceeds the assigned-worker limit.".into(),
                    )
                } else {
                    PluginResourceInput::Catalog(names)
                }
            }
            Err(_) => {
                PluginResourceInput::Unavailable("Plugin catalog could not be prepared.".into())
            }
        },
        ResourceRequest::Prepare { plugin } => {
            if store
                .prepared_plugin_manifest(r, plugin)
                .map_err(unavailable)?
                .is_some()
            {
                PluginResourceInput::Retained
            } else {
                match state.plugins.snapshot(plugin, &r.workspace_id).await {
                    Ok(mut snapshot) => {
                        // Text resources are redacted before hashing. Binary resources containing
                        // a known secret are rejected rather than corrupted during transfer.
                        let host = tool_runtime::host_redactor(state);
                        let worker = beale_tools::Redactor::new([bearer.to_owned()]);
                        let mut private = false;
                        for resource in &mut snapshot.resources {
                            let text = String::from_utf8_lossy(&resource.bytes);
                            let redacted = host.redact(&text);
                            let redacted = worker.redact(&redacted);
                            if redacted != text {
                                if std::str::from_utf8(&resource.bytes).is_err() {
                                    private = true;
                                    break;
                                }
                                resource.bytes = redacted.as_bytes().to_vec();
                                resource.file.size = resource.bytes.len() as u64;
                                resource.file.sha256 =
                                    beale_storage::swarm_files::hash(&resource.bytes);
                            }
                        }
                        snapshot.manifest.files =
                            snapshot.resources.iter().map(|r| r.file.clone()).collect();
                        if private || snapshot.validate().is_err() {
                            PluginResourceInput::Unavailable(
                                "Plugin resources could not be safely published.".into(),
                            )
                        } else {
                            PluginResourceInput::Snapshot(PreparedPluginBundle {
                                manifest: snapshot.manifest,
                                bytes: snapshot.resources.into_iter().map(|r| r.bytes).collect(),
                            })
                        }
                    }
                    Err(_) => PluginResourceInput::Unavailable(
                        "Plugin resources are unavailable or incompatible with this workspace."
                            .into(),
                    ),
                }
            }
        }
        ResourceRequest::Read { .. } => PluginResourceInput::Retained,
    };
    let files = crate::research_files::store(state).map_err(unavailable)?;
    store
        .worker_plugin_resources(&files, bearer, r, tool_runtime::unix_ms(), input)
        .map_err(unavailable)
}
